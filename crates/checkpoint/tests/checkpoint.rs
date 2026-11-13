use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use xm_relay_checkpoint::{
    compare_with_workspace, create_checkpoint, inspect_checkpoint, list_checkpoints,
};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("xmr-ckpt-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let s = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(s.success());
}

#[test]
fn creates_verifies_redacts_and_detects_drift() {
    let t = Tmp::new("all");
    let repo = t.0.join("repo");
    let store = t.0.join("store");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("a.txt"), "hello\n").unwrap();
    fs::write(repo.join("AGENTS.md"), "be careful\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);

    // Dirty the tree with a fake secret in tracked diff and an untracked .env
    let tok = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    fs::write(repo.join("a.txt"), format!("hello\nkey={tok}\n")).unwrap();
    fs::write(repo.join(".env"), "X=1\n").unwrap();
    fs::write(repo.join("notes.txt"), "v1\n").unwrap();

    let m = create_checkpoint(&repo, &store).unwrap();
    assert!(m.working_tree_dirty);
    assert_eq!(m.branch.as_deref(), Some("main"));
    assert_eq!(m.patch_redactions.len(), 1);
    assert_eq!(m.sensitive_files.len(), 1);
    assert_eq!(m.instruction_files, ["AGENTS.md"]);

    // Verify patch on disk does NOT contain the raw secret
    let patch_on_disk =
        fs::read_to_string(store.join(&m.checkpoint_id).join("working-tree.patch")).unwrap();
    assert!(!patch_on_disk.contains(&tok));
    assert!(patch_on_disk.contains("[REDACTED:aws_access_key]"));

    let list = list_checkpoints(&store, None).unwrap();
    assert_eq!(list.len(), 1);

    let detail = inspect_checkpoint(&store, &m.checkpoint_id).unwrap();
    assert!(detail.checksums_valid);

    // Immediately comparing with workspace shows no drift
    let drift0 = compare_with_workspace(&store, &m.checkpoint_id, Some(&repo)).unwrap();
    assert!(drift0.clean_match);

    // Mutate untracked file and branch -> drift is detected without touching files
    fs::write(repo.join("notes.txt"), "v2\n").unwrap();
    fs::write(repo.join("extra.txt"), "new\n").unwrap();
    git(&repo, &["checkout", "-q", "-b", "feat/x"]);
    let drift1 = compare_with_workspace(&store, &m.checkpoint_id, Some(&repo)).unwrap();
    assert!(!drift1.clean_match);
    assert_eq!(
        drift1.branch_changed,
        Some((Some("main".into()), Some("feat/x".into())))
    );
    assert_eq!(drift1.untracked_modified, ["notes.txt"]);
    assert_eq!(drift1.untracked_added, ["extra.txt"]);

    // Tamper with artifact -> checksum verification catches it
    fs::write(
        store.join(&m.checkpoint_id).join("working-tree.patch"),
        "tampered",
    )
    .unwrap();
    let tampered = inspect_checkpoint(&store, &m.checkpoint_id).unwrap();
    assert!(!tampered.checksums_valid);
    assert!(compare_with_workspace(&store, &m.checkpoint_id, Some(&repo)).is_err());
}
