//! Git & Workspace Checkpoint creation, inspection, and drift comparison (spec section 11).
//!
//! Never modifies the working tree or Git state. Tracked diffs are redacted of
//! detected secrets before writing to disk.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use xm_relay_repository::{
    discover_instructions, inspect, sha256_hex, tracked_patch, InstructionSet, RepoError,
    RepositoryState,
};
use xm_relay_security::{redact_text, sensitive_file, Finding, SensitiveFile};

pub const SCHEMA_VERSION: &str = "1.0";

#[derive(Debug)]
pub enum CheckpointError {
    NotARepository(String),
    NotFound(String),
    Repo(RepoError),
    Io(String),
    Integrity(String),
}

impl fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotARepository(p) => write!(f, "not inside a Git repository: {p}"),
            Self::NotFound(id) => write!(f, "checkpoint not found: {id}"),
            Self::Repo(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Integrity(e) => write!(f, "integrity error: {e}"),
        }
    }
}

impl std::error::Error for CheckpointError {}

impl From<RepoError> for CheckpointError {
    fn from(e: RepoError) -> Self {
        Self::Repo(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDigest {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub schema_version: String,
    pub checkpoint_id: String,
    pub created_at: String,
    pub project_root: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub head_commit: Option<String>,
    pub working_tree_dirty: bool,
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub untracked_count: usize,
    pub conflicted_count: usize,
    pub instruction_files: Vec<String>,
    pub patch_redactions: Vec<Finding>,
    pub sensitive_files: Vec<SensitiveFile>,
    pub artifacts: Vec<ArtifactDigest>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointDetail {
    pub manifest: CheckpointManifest,
    pub repository_state: RepositoryState,
    pub instructions: InstructionSet,
    pub checksums_valid: bool,
    pub checksum_errors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDrift {
    pub checkpoint_id: String,
    pub clean_match: bool,
    pub branch_changed: Option<(Option<String>, Option<String>)>,
    pub head_changed: Option<(Option<String>, Option<String>)>,
    pub patch_sha256_changed: bool,
    pub untracked_added: Vec<String>,
    pub untracked_removed: Vec<String>,
    pub untracked_modified: Vec<String>,
}

/// Default storage location: `$XDG_DATA_HOME/xm-relay/checkpoints` or `~/.local/share/xm-relay/checkpoints`.
pub fn default_store_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))?;
    Some(base.join("xm-relay/checkpoints"))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '\\']) && !id.contains("..")
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn millis_to_rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn generate_id(store_dir: &Path, ms: i64) -> String {
    let base = format!("ckpt_{ms}_{}", std::process::id());
    if !store_dir.join(&base).exists() {
        return base;
    }
    for n in 1..1000 {
        let cand = format!("{base}_{n}");
        if !store_dir.join(&cand).exists() {
            return cand;
        }
    }
    base
}

fn io_err(e: impl fmt::Display) -> CheckpointError {
    CheckpointError::Io(e.to_string())
}

fn write_artifact(
    dir: &Path,
    rel: &str,
    bytes: &[u8],
    artifacts: &mut Vec<ArtifactDigest>,
) -> Result<(), CheckpointError> {
    let path = dir.join(rel);
    fs::write(&path, bytes).map_err(io_err)?;
    artifacts.push(ArtifactDigest {
        path: rel.into(),
        size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    });
    Ok(())
}

/// Create a read-only workspace checkpoint in `store_dir/<checkpoint_id>/`.
pub fn create_checkpoint(
    project_path: &Path,
    store_dir: &Path,
) -> Result<CheckpointManifest, CheckpointError> {
    let repo_state = inspect(project_path)?;
    if !repo_state.is_repository {
        return Err(CheckpointError::NotARepository(
            project_path.display().to_string(),
        ));
    }
    let root_str = repo_state.root.clone().unwrap_or_default();
    let root = PathBuf::from(&root_str);

    let mut instructions = discover_instructions(&root);
    let mut warnings = repo_state.warnings.clone();
    warnings.extend(instructions.warnings.clone());

    let raw_patch = tracked_patch(&root)?;
    let patch_str = String::from_utf8_lossy(&raw_patch);
    let (redacted_patch, patch_redactions) = redact_text(&patch_str);
    if !patch_redactions.is_empty() {
        warnings.push(format!(
            "redacted {} potential secret(s) from working-tree.patch",
            patch_redactions.len()
        ));
    }

    for f in &mut instructions.files {
        if let Some(c) = &f.content {
            let (redacted, findings) = redact_text(c);
            if !findings.is_empty() {
                warnings.push(format!(
                    "redacted {} potential secret(s) from instruction file {}",
                    findings.len(),
                    f.path
                ));
                f.content = Some(redacted);
            }
        }
    }

    let mut names: Vec<&str> = repo_state
        .untracked
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    names.extend(
        repo_state
            .staged
            .iter()
            .chain(&repo_state.unstaged)
            .map(|f| f.path.as_str()),
    );
    names.sort();
    names.dedup();
    let sensitive_files: Vec<SensitiveFile> =
        names.into_iter().filter_map(sensitive_file).collect();
    for s in &sensitive_files {
        warnings.push(format!(
            "sensitive file present in workspace: {} ({})",
            s.path, s.reason
        ));
    }

    fs::create_dir_all(store_dir).map_err(io_err)?;
    let ms = now_millis();
    let id = generate_id(store_dir, ms);
    let ckpt_dir = store_dir.join(&id);
    fs::create_dir_all(&ckpt_dir).map_err(io_err)?;

    let mut artifacts = Vec::new();
    let repo_json = serde_json::to_vec_pretty(&repo_state).map_err(io_err)?;
    write_artifact(
        &ckpt_dir,
        "repository-state.json",
        &repo_json,
        &mut artifacts,
    )?;

    let ins_json = serde_json::to_vec_pretty(&instructions).map_err(io_err)?;
    write_artifact(&ckpt_dir, "instructions.json", &ins_json, &mut artifacts)?;

    write_artifact(
        &ckpt_dir,
        "working-tree.patch",
        redacted_patch.as_bytes(),
        &mut artifacts,
    )?;

    let manifest = CheckpointManifest {
        schema_version: SCHEMA_VERSION.into(),
        checkpoint_id: id,
        created_at: millis_to_rfc3339(ms),
        project_root: root_str,
        branch: repo_state.branch.clone(),
        detached: repo_state.detached,
        head_commit: repo_state.head_commit.clone(),
        working_tree_dirty: repo_state.working_tree_dirty,
        staged_count: repo_state.staged.len(),
        unstaged_count: repo_state.unstaged.len(),
        untracked_count: repo_state.untracked.len(),
        conflicted_count: repo_state.conflicted.len(),
        instruction_files: instructions.files.iter().map(|f| f.path.clone()).collect(),
        patch_redactions,
        sensitive_files,
        artifacts: artifacts.clone(),
        warnings,
    };

    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(io_err)?;
    fs::write(ckpt_dir.join("manifest.json"), &manifest_bytes).map_err(io_err)?;

    let mut checksum_lines: Vec<String> = artifacts
        .iter()
        .map(|a| format!("{}  {}", a.sha256, a.path))
        .collect();
    checksum_lines.push(format!("{}  manifest.json", sha256_hex(&manifest_bytes)));
    let checksums_txt = format!("{}\n", checksum_lines.join("\n"));
    fs::write(ckpt_dir.join("checksums.txt"), checksums_txt).map_err(io_err)?;

    Ok(manifest)
}

/// List checkpoints in `store_dir`, newest first.
pub fn list_checkpoints(
    store_dir: &Path,
    project_root: Option<&str>,
) -> Result<Vec<CheckpointManifest>, CheckpointError> {
    if !store_dir.exists() {
        return Ok(Vec::new());
    }
    let rd = fs::read_dir(store_dir).map_err(io_err)?;
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let mpath = entry.path().join("manifest.json");
        if !mpath.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(&mpath) else {
            continue;
        };
        let Ok(m) = serde_json::from_slice::<CheckpointManifest>(&bytes) else {
            continue;
        };
        if project_root.is_some_and(|p| m.project_root != p) {
            continue;
        }
        out.push(m);
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

/// Load a checkpoint and verify `checksums.txt` against on-disk files.
pub fn inspect_checkpoint(store_dir: &Path, id: &str) -> Result<CheckpointDetail, CheckpointError> {
    if !valid_id(id) {
        return Err(CheckpointError::NotFound(id.into()));
    }
    let dir = store_dir.join(id);
    if !dir.is_dir() {
        return Err(CheckpointError::NotFound(id.into()));
    }
    let manifest: CheckpointManifest =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).map_err(io_err)?)
            .map_err(io_err)?;
    let repository_state: RepositoryState =
        serde_json::from_slice(&fs::read(dir.join("repository-state.json")).map_err(io_err)?)
            .map_err(io_err)?;
    let instructions: InstructionSet =
        serde_json::from_slice(&fs::read(dir.join("instructions.json")).map_err(io_err)?)
            .map_err(io_err)?;

    let mut errors = Vec::new();
    match fs::read_to_string(dir.join("checksums.txt")) {
        Ok(txt) => {
            for line in txt.lines().filter(|l| !l.trim().is_empty()) {
                let Some((expected, rel)) = line.split_once("  ") else {
                    errors.push(format!("malformed checksum line: {line}"));
                    continue;
                };
                if !valid_id(rel) {
                    errors.push(format!("invalid path in checksums.txt: {rel}"));
                    continue;
                }
                match fs::read(dir.join(rel)) {
                    Ok(bytes) => {
                        let actual = sha256_hex(&bytes);
                        if actual != expected {
                            errors.push(format!("checksum mismatch for {rel}"));
                        }
                    }
                    Err(e) => errors.push(format!("missing artifact {rel}: {e}")),
                }
            }
        }
        Err(e) => errors.push(format!("cannot read checksums.txt: {e}")),
    }

    Ok(CheckpointDetail {
        manifest,
        repository_state,
        instructions,
        checksums_valid: errors.is_empty(),
        checksum_errors: errors,
    })
}

/// Compare the current live repository state with a stored checkpoint (spec 11.5).
/// Never modifies the workspace.
pub fn compare_with_workspace(
    store_dir: &Path,
    id: &str,
    live_path: Option<&Path>,
) -> Result<WorkspaceDrift, CheckpointError> {
    let detail = inspect_checkpoint(store_dir, id)?;
    if !detail.checksums_valid {
        return Err(CheckpointError::Integrity(
            detail.checksum_errors.join("; "),
        ));
    }
    let root = live_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&detail.manifest.project_root));
    let live = inspect(&root)?;
    if !live.is_repository {
        return Err(CheckpointError::NotARepository(root.display().to_string()));
    }

    let branch_changed = (detail.repository_state.branch != live.branch)
        .then(|| (detail.repository_state.branch.clone(), live.branch.clone()));
    let head_changed = (detail.repository_state.head_commit != live.head_commit).then(|| {
        (
            detail.repository_state.head_commit.clone(),
            live.head_commit.clone(),
        )
    });

    let live_raw_patch = tracked_patch(&root)?;
    let (live_redacted, _) = redact_text(&String::from_utf8_lossy(&live_raw_patch));
    let live_patch_sha = sha256_hex(live_redacted.as_bytes());
    let ckpt_patch_sha = detail
        .manifest
        .artifacts
        .iter()
        .find(|a| a.path == "working-tree.patch")
        .map(|a| a.sha256.as_str())
        .unwrap_or("");
    let patch_sha256_changed = live_patch_sha != ckpt_patch_sha;

    let old_un: BTreeMap<&str, Option<&str>> = detail
        .repository_state
        .untracked
        .iter()
        .map(|f| (f.path.as_str(), f.sha256.as_deref()))
        .collect();
    let new_un: BTreeMap<&str, Option<&str>> = live
        .untracked
        .iter()
        .map(|f| (f.path.as_str(), f.sha256.as_deref()))
        .collect();

    let mut untracked_added = Vec::new();
    let mut untracked_removed = Vec::new();
    let mut untracked_modified = Vec::new();

    for (p, new_h) in &new_un {
        match old_un.get(p) {
            None => untracked_added.push((*p).to_string()),
            Some(old_h) if old_h != new_h => untracked_modified.push((*p).to_string()),
            _ => {}
        }
    }
    for p in old_un.keys() {
        if !new_un.contains_key(p) {
            untracked_removed.push((*p).to_string());
        }
    }

    let clean_match = branch_changed.is_none()
        && head_changed.is_none()
        && !patch_sha256_changed
        && untracked_added.is_empty()
        && untracked_removed.is_empty()
        && untracked_modified.is_empty();

    Ok(WorkspaceDrift {
        checkpoint_id: id.into(),
        clean_match,
        branch_changed,
        head_changed,
        patch_sha256_changed,
        untracked_added,
        untracked_removed,
        untracked_modified,
    })
}
