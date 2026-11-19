//! Read-only Git/workspace inspection via the `git` CLI (spec section 11).
//!
//! Nothing here mutates the repository: no reset, clean, checkout, stash, commit
//! or index refresh. `GIT_OPTIONAL_LOCKS=0` stops `git status` from rewriting the index.

pub mod instructions;

pub use instructions::{discover_instructions, InstructionFile, InstructionKind, InstructionSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Untracked files above this size are listed but not hashed.
const MAX_HASH_BYTES: u64 = 5 * 1024 * 1024;
/// Working-tree patches above this size are truncated before redaction and
/// packaging, so a repo with multi-GB binary churn cannot exhaust memory.
pub const MAX_PATCH_BYTES: usize = 64 * 1024 * 1024;
const RECENT_COMMITS: usize = 10;

#[derive(Debug)]
pub enum RepoError {
    GitUnavailable(String),
    GitFailed { args: String, stderr: String },
    Parse(String),
}

impl fmt::Display for RepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitUnavailable(m) => write!(f, "git is not available: {m}"),
            Self::GitFailed { args, stderr } => write!(f, "git {args} failed: {}", stderr.trim()),
            Self::Parse(m) => write!(f, "could not parse git output: {m}"),
        }
    }
}

impl std::error::Error for RepoError {}

/// Redact free-text fields of a repository state in place and return the number
/// of redactions. Paths are untouched (they index git artifacts), but branch
/// names, upstream refs, commit subjects, and warnings are user-controlled text
/// that can carry pasted secrets.
pub fn redact_free_text(state: &mut RepositoryState) -> usize {
    let mut count = 0;
    for slot in [&mut state.branch, &mut state.upstream] {
        if let Some(s) = slot {
            let (clean, f) = xmrelay_security::redact_text(s);
            count += f.len();
            *slot = Some(clean);
        }
    }
    for c in &mut state.recent_commits {
        let (clean, f) = xmrelay_security::redact_text(&c.subject);
        count += f.len();
        c.subject = clean;
    }
    for w in &mut state.warnings {
        let (clean, f) = xmrelay_security::redact_text(w);
        count += f.len();
        *w = clean;
    }
    count
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub orig_path: Option<String>,
    /// Git status letter: M, A, D, R, C, T.
    pub status: char,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntrackedFile {
    pub path: String,
    pub size: Option<u64>,
    pub is_symlink: bool,
    /// `None` when not hashed (too large, symlink, or unreadable); see warnings.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStat {
    pub path: String,
    /// `None` for binary files.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub files: Vec<DiffStat>,
    pub additions: u64,
    pub deletions: u64,
    pub binary_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub subject: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleInfo {
    pub path: String,
    pub commit: String,
    /// `-` uninitialised, `+` differs from recorded commit, `U` conflicts, ` ` clean.
    pub state: char,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryState {
    pub is_repository: bool,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub head_commit: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub working_tree_dirty: bool,
    pub staged: Vec<ChangedFile>,
    pub unstaged: Vec<ChangedFile>,
    pub conflicted: Vec<String>,
    pub untracked: Vec<UntrackedFile>,
    pub diff_summary: DiffSummary,
    pub recent_commits: Vec<CommitInfo>,
    pub submodules: Vec<SubmoduleInfo>,
    pub worktrees: Vec<WorktreeInfo>,
    pub warnings: Vec<String>,
}

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, RepoError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["--no-pager", "-c", "core.quotepath=off"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| RepoError::GitUnavailable(e.to_string()))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(RepoError::GitFailed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

fn git_text(dir: &Path, args: &[&str]) -> Result<String, RepoError> {
    Ok(String::from_utf8_lossy(&git(dir, args)?)
        .trim_end()
        .to_string())
}

/// Inspect the repository containing `path`. A non-repository yields
/// `is_repository: false` rather than an error.
pub fn inspect(path: &Path) -> Result<RepositoryState, RepoError> {
    let mut st = RepositoryState::default();
    let root = match git_text(path, &["rev-parse", "--show-toplevel"]) {
        Ok(r) => PathBuf::from(r),
        Err(RepoError::GitFailed { .. }) => return Ok(st),
        Err(e) => return Err(e),
    };
    st.is_repository = true;
    st.root = Some(root.display().to_string());

    st.head_commit = git_text(&root, &["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .filter(|s| !s.is_empty());
    parse_status(&root, &mut st)?;
    st.working_tree_dirty = !(st.staged.is_empty()
        && st.unstaged.is_empty()
        && st.conflicted.is_empty()
        && st.untracked.is_empty());

    st.diff_summary = diff_summary(&root, st.head_commit.is_some())?;
    if st.head_commit.is_some() {
        let fmt = "--format=%H%x1f%s%x1f%cI";
        let log = git_text(&root, &["log", &format!("-n{RECENT_COMMITS}"), fmt])?;
        st.recent_commits = log
            .lines()
            .filter_map(|l| {
                let mut p = l.split('\u{1f}');
                Some(CommitInfo {
                    hash: p.next()?.into(),
                    subject: p.next()?.into(),
                    date: p.next()?.into(),
                })
            })
            .collect();
    }
    st.submodules = git_text(&root, &["submodule", "status"])
        .unwrap_or_default()
        .lines()
        .filter_map(parse_submodule_line)
        .collect();
    st.worktrees =
        parse_worktrees(&git_text(&root, &["worktree", "list", "--porcelain"]).unwrap_or_default());
    hash_untracked(&root, &mut st);
    Ok(st)
}

fn status_char(c: char) -> Option<char> {
    (c != '.').then_some(c)
}

/// Parse one `git submodule status` line: `<status><40-hex> <path>[ (desc)]`.
/// Field-split instead of fixed byte offsets so a format tweak degrades to a
/// skipped line instead of slicing garbage out of the middle.
fn parse_submodule_line(l: &str) -> Option<SubmoduleInfo> {
    let mut chars = l.chars();
    let state = chars.next()?;
    let rest = chars.as_str();
    let mut it = rest.splitn(2, ' ');
    let commit = it.next()?.to_string();
    let mut path = it.next()?.to_string();
    // Drop git's optional " (ref)" description suffix.
    if let Some((p, _)) = path.split_once(" (") {
        if !p.is_empty() {
            path = p.to_string();
        }
    }
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) || path.is_empty() {
        return None;
    }
    Some(SubmoduleInfo {
        state,
        commit,
        path,
    })
}

fn parse_status(root: &Path, st: &mut RepositoryState) -> Result<(), RepoError> {
    let raw = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
    )?;
    let text = String::from_utf8_lossy(&raw);
    for rec in text.split('\0').filter(|r| !r.is_empty()) {
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.head" if v == "(detached)" => st.detached = true,
                "branch.head" => st.branch = Some(v.into()),
                "branch.upstream" => st.upstream = Some(v.into()),
                "branch.ab" => {
                    let mut p = v.split(' ');
                    st.ahead = p
                        .next()
                        .and_then(|a| a.trim_start_matches('+').parse().ok());
                    st.behind = p
                        .next()
                        .and_then(|b| b.trim_start_matches('-').parse().ok());
                }
                _ => {}
            }
            continue;
        }
        let mut chars = rec.chars();
        match chars.next() {
            Some('1') => {
                let f: Vec<&str> = rec.splitn(9, ' ').collect();
                if f.len() < 9 {
                    return Err(RepoError::Parse(rec.into()));
                }
                let mut xy = f[1].chars();
                let (x, y) = (xy.next().unwrap_or('.'), xy.next().unwrap_or('.'));
                if let Some(s) = status_char(x) {
                    st.staged.push(ChangedFile {
                        path: f[8].into(),
                        orig_path: None,
                        status: s,
                    });
                }
                if let Some(s) = status_char(y) {
                    st.unstaged.push(ChangedFile {
                        path: f[8].into(),
                        orig_path: None,
                        status: s,
                    });
                }
            }
            Some('u') => {
                let f: Vec<&str> = rec.splitn(11, ' ').collect();
                if f.len() < 11 {
                    return Err(RepoError::Parse(rec.into()));
                }
                st.conflicted.push(f[10].into());
            }
            Some('?') => st.untracked.push(UntrackedFile {
                path: rec[2..].into(),
                size: None,
                is_symlink: false,
                sha256: None,
            }),
            _ => st
                .warnings
                .push(format!("unrecognised git status record: {rec}")),
        }
    }
    Ok(())
}

fn diff_summary(root: &Path, has_head: bool) -> Result<DiffSummary, RepoError> {
    let mut args = vec![
        "diff",
        "--numstat",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
    ];
    if has_head {
        args.push("HEAD");
    } else {
        args.push("--cached");
    }
    let raw = git(root, &args)?;
    let mut s = DiffSummary::default();
    for rec in String::from_utf8_lossy(&raw)
        .split('\0')
        .filter(|r| !r.is_empty())
    {
        let mut p = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (p.next(), p.next(), p.next()) else {
            continue;
        };
        let (additions, deletions) = (a.parse::<u64>().ok(), d.parse::<u64>().ok());
        match (additions, deletions) {
            (Some(a), Some(d)) => {
                s.additions += a;
                s.deletions += d;
            }
            _ => s.binary_files += 1,
        }
        s.files.push(DiffStat {
            path: path.into(),
            additions,
            deletions,
        });
    }
    Ok(s)
}

fn parse_worktrees(text: &str) -> Vec<WorktreeInfo> {
    let mut out = Vec::new();
    for block in text.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut w = WorktreeInfo {
            path: String::new(),
            head: None,
            branch: None,
        };
        for l in block.lines() {
            if let Some(p) = l.strip_prefix("worktree ") {
                w.path = p.into();
            } else if let Some(h) = l.strip_prefix("HEAD ") {
                w.head = Some(h.into());
            } else if let Some(b) = l.strip_prefix("branch ") {
                w.branch = Some(b.trim_start_matches("refs/heads/").into());
            }
        }
        if !w.path.is_empty() {
            out.push(w);
        }
    }
    out
}

fn hash_untracked(root: &Path, st: &mut RepositoryState) {
    let mut warnings = Vec::new();
    for f in &mut st.untracked {
        let p = root.join(&f.path);
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            warnings.push(format!("cannot stat untracked file: {}", f.path));
            continue;
        };
        f.is_symlink = meta.file_type().is_symlink();
        f.size = Some(meta.len());
        if f.is_symlink {
            continue; // never follow symlinks (spec 16.3)
        }
        if !meta.is_file() {
            // FIFOs block on read until a writer appears; sockets error. Skip
            // both so a stray pipe in the workspace cannot hang inspection.
            warnings.push(format!("untracked special file not hashed: {}", f.path));
            continue;
        }
        if meta.len() > MAX_HASH_BYTES {
            warnings.push(format!(
                "untracked file not hashed (over {MAX_HASH_BYTES} bytes): {}",
                f.path
            ));
            continue;
        }
        match std::fs::read(&p) {
            Ok(bytes) => f.sha256 = Some(sha256_hex(&bytes)),
            Err(e) => warnings.push(format!("cannot read untracked file {}: {e}", f.path)),
        }
    }
    st.warnings.extend(warnings);
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Binary-safe patch of all tracked changes (staged and unstaged) relative to HEAD.
/// Untracked files are NOT included (spec 11.4); use `RepositoryState::untracked`.
pub fn tracked_patch(root: &Path) -> Result<Vec<u8>, RepoError> {
    let has_head = git_text(root, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    let mut args = vec![
        "diff",
        "--binary",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
    ];
    args.push(if has_head { "HEAD" } else { "--cached" });
    git(root, &args)
}

/// Top-level directory of the repository containing `path`, if any.
pub fn repo_root(path: &Path) -> Option<PathBuf> {
    git_text(path, &["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod submodule_tests {
    use super::*;

    #[test]
    fn parses_submodule_status_lines() {
        let normal =
            parse_submodule_line(" a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0 sub/one (heads/main)")
                .unwrap();
        assert_eq!(normal.state, ' ');
        assert_eq!(normal.commit, "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0");
        assert_eq!(normal.path, "sub/one");

        // Modified submodule without a description suffix.
        let modified =
            parse_submodule_line("+0123456789abcdef0123456789abcdef01234567 sub/two").unwrap();
        assert_eq!(modified.state, '+');
        assert_eq!(modified.path, "sub/two");

        // Garbage lines (empty, short hashes, missing path) are skipped.
        assert!(parse_submodule_line("").is_none());
        assert!(parse_submodule_line("-zzzz sub/x").is_none());
        assert!(parse_submodule_line(" a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0 ").is_none());
    }
}
