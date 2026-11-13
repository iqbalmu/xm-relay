//! Project instruction discovery (`AGENTS.md`, `CLAUDE.md`, ...).
//!
//! Content is untrusted data (spec 16.4): it is recorded with provenance and never
//! executed or treated as instructions for this tool. Symlinks are not followed.

use crate::{repo_root, sha256_hex};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_CONTENT_BYTES: u64 = 256 * 1024;
const MAX_DEPTH: usize = 4;
const MAX_FILES: usize = 100;
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    ".venv",
    "__pycache__",
];

/// (relative path, kind). Only widely documented names; extend with evidence.
const KNOWN: &[(&str, InstructionKind)] = &[
    ("AGENTS.md", InstructionKind::Agents),
    ("CLAUDE.md", InstructionKind::Claude),
    ("CLAUDE.local.md", InstructionKind::Claude),
    ("GEMINI.md", InstructionKind::Gemini),
    (".cursorrules", InstructionKind::Cursor),
    (".github/copilot-instructions.md", InstructionKind::Copilot),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionKind {
    Agents,
    Claude,
    Gemini,
    Cursor,
    Copilot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionFile {
    /// Path relative to the project root, with `/` separators.
    pub path: String,
    pub kind: InstructionKind,
    /// Directory the file governs, relative to the root (`""` is the root).
    pub scope: String,
    /// True when `scope` contains (or equals) the directory discovery started from.
    pub applies_to_start: bool,
    pub size: u64,
    pub is_symlink: bool,
    pub sha256: Option<String>,
    pub content: Option<String>,
    /// Content was cut at the size limit; the hash covers the full file only if false.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionSet {
    pub root: String,
    pub files: Vec<InstructionFile>,
    pub warnings: Vec<String>,
}

pub fn discover_instructions(start: &Path) -> InstructionSet {
    let start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let root = repo_root(&start).unwrap_or_else(|| start.clone());
    let mut set = InstructionSet {
        root: root.display().to_string(),
        ..Default::default()
    };
    walk(&root, &root, &start, 0, &mut set);
    // Root scope first, then shallower scopes, then by path.
    set.files.sort_by_key(|f| {
        (
            !f.scope.is_empty(),
            f.scope.matches('/').count(),
            f.path.clone(),
        )
    });
    set
}

fn walk(root: &Path, dir: &Path, start: &Path, depth: usize, set: &mut InstructionSet) {
    for (name, kind) in KNOWN {
        let p = dir.join(name);
        if std::fs::symlink_metadata(&p).is_ok() {
            if set.files.len() >= MAX_FILES {
                if !set.warnings.iter().any(|w| w.contains("limit")) {
                    set.warnings.push(format!(
                        "instruction file limit ({MAX_FILES}) reached; discovery is incomplete"
                    ));
                }
                return;
            }
            read_file(root, dir, &p, *kind, start, set);
        }
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) // symlinked dirs are not dirs here
        .filter(|e| !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        .map(|e| e.path())
        .collect();
    subdirs.sort();
    for d in subdirs {
        walk(root, &d, start, depth + 1, set);
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn read_file(
    root: &Path,
    dir: &Path,
    p: &Path,
    kind: InstructionKind,
    start: &Path,
    set: &mut InstructionSet,
) {
    let meta = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) => {
            set.warnings
                .push(format!("cannot stat {}: {e}", rel(root, p)));
            return;
        }
    };
    let path = rel(root, p);
    let mut f = InstructionFile {
        path: path.clone(),
        kind,
        scope: rel(root, dir),
        applies_to_start: start.starts_with(dir),
        size: meta.len(),
        is_symlink: meta.file_type().is_symlink(),
        sha256: None,
        content: None,
        truncated: false,
    };
    if f.is_symlink {
        set.warnings
            .push(format!("{path} is a symlink and was not followed"));
    } else if !meta.is_file() {
        set.warnings.push(format!("{path} is not a regular file"));
    } else {
        match std::fs::read(p) {
            Ok(bytes) => {
                f.sha256 = Some(sha256_hex(&bytes));
                f.truncated = meta.len() > MAX_CONTENT_BYTES;
                let cut = &bytes[..bytes.len().min(MAX_CONTENT_BYTES as usize)];
                match std::str::from_utf8(cut) {
                    Ok(s) => f.content = Some(s.to_string()),
                    // A cut inside a multi-byte char is fine; anything else is not text.
                    Err(e) if f.truncated && e.error_len().is_none() => {
                        f.content =
                            Some(String::from_utf8_lossy(&cut[..e.valid_up_to()]).into_owned())
                    }
                    Err(_) => set
                        .warnings
                        .push(format!("{path} is not valid UTF-8; content omitted")),
                }
                if f.truncated {
                    set.warnings.push(format!(
                        "{path} exceeds {MAX_CONTENT_BYTES} bytes; content truncated"
                    ));
                }
            }
            Err(e) => set.warnings.push(format!("cannot read {path}: {e}")),
        }
    }
    set.files.push(f);
}
