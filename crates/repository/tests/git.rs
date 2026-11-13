use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use xm_relay_repository::{inspect, sha256_hex, tracked_patch};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("xmr-git-{}-{tag}", std::process::id()));
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
    assert!(s.success(), "git {args:?}");
}

fn repo(tag: &str) -> Tmp {
    let t = Tmp::new(tag);
    git(&t.0, &["init", "-q", "-b", "main"]);
    fs::write(t.0.join("a.txt"), "one\ntwo\n").unwrap();
    fs::write(t.0.join("b.txt"), "b\n").unwrap();
    git(&t.0, &["add", "."]);
    git(&t.0, &["commit", "-q", "-m", "first"]);
    t
}

#[test]
fn non_repository_is_not_an_error() {
    let t = Tmp::new("none");
    let s = inspect(&t.0).unwrap();
    assert!(!s.is_repository);
}

#[test]
fn clean_repo() {
    let t = repo("clean");
    let s = inspect(&t.0).unwrap();
    assert!(s.is_repository && !s.working_tree_dirty && !s.detached);
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.head_commit.as_ref().map(String::len), Some(40));
    assert_eq!(s.recent_commits.len(), 1);
    assert_eq!(s.recent_commits[0].subject, "first");
    assert_eq!(s.worktrees.len(), 1);
}

#[test]
fn dirty_repo_separates_staged_unstaged_untracked() {
    let t = repo("dirty");
    fs::write(t.0.join("a.txt"), "one\ntwo\nthree\n").unwrap(); // unstaged
    fs::write(t.0.join("b.txt"), "changed\n").unwrap();
    git(&t.0, &["add", "b.txt"]); // staged
    fs::create_dir_all(t.0.join("sub")).unwrap();
    fs::write(t.0.join("sub/new file.txt"), "hello").unwrap(); // untracked with space
    fs::write(t.0.join("bin.dat"), [0u8, 1, 2, 0]).unwrap();
    let s = inspect(&t.0).unwrap();
    assert!(s.working_tree_dirty);
    assert_eq!(
        s.staged
            .iter()
            .map(|f| (f.path.as_str(), f.status))
            .collect::<Vec<_>>(),
        [("b.txt", 'M')]
    );
    assert_eq!(
        s.unstaged
            .iter()
            .map(|f| (f.path.as_str(), f.status))
            .collect::<Vec<_>>(),
        [("a.txt", 'M')]
    );
    let mut un: Vec<_> = s.untracked.iter().map(|f| f.path.as_str()).collect();
    un.sort();
    assert_eq!(un, ["bin.dat", "sub/new file.txt"]);
    let f = s
        .untracked
        .iter()
        .find(|f| f.path == "sub/new file.txt")
        .unwrap();
    assert_eq!(f.sha256.as_deref(), Some(sha256_hex(b"hello").as_str()));
    // diff vs HEAD covers both tracked changes
    assert_eq!(s.diff_summary.files.len(), 2);
    assert_eq!(s.diff_summary.additions, 2);
    assert_eq!(s.diff_summary.deletions, 1);
    // patch is tracked-only and does not contain untracked content
    let patch = String::from_utf8(tracked_patch(&t.0).unwrap()).unwrap();
    assert!(patch.contains("a.txt") && patch.contains("b.txt") && !patch.contains("new file"));
}

#[test]
fn detached_head_and_no_commits() {
    let t = repo("detached");
    git(&t.0, &["checkout", "-q", "--detach"]);
    let s = inspect(&t.0).unwrap();
    assert!(s.detached && s.branch.is_none() && s.head_commit.is_some());

    let e = Tmp::new("empty");
    git(&e.0, &["init", "-q", "-b", "main"]);
    fs::write(e.0.join("x"), "x").unwrap();
    git(&e.0, &["add", "x"]);
    let s = inspect(&e.0).unwrap();
    assert!(s.is_repository && s.head_commit.is_none());
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.staged.len(), 1);
    assert_eq!(s.diff_summary.files.len(), 1); // staged diff without HEAD
}

#[test]
fn conflicts_are_reported() {
    let t = repo("conflict");
    git(&t.0, &["checkout", "-q", "-b", "other"]);
    fs::write(t.0.join("a.txt"), "other\n").unwrap();
    git(&t.0, &["commit", "-qam", "other"]);
    git(&t.0, &["checkout", "-q", "main"]);
    fs::write(t.0.join("a.txt"), "main\n").unwrap();
    git(&t.0, &["commit", "-qam", "main"]);
    let _ = Command::new("git")
        .arg("-C")
        .arg(&t.0)
        .args(["merge", "other"])
        .output();
    let s = inspect(&t.0).unwrap();
    assert_eq!(s.conflicted, ["a.txt"]);
    assert!(s.working_tree_dirty);
}

#[test]
fn inspection_does_not_modify_the_repository() {
    let t = repo("readonly");
    fs::write(t.0.join("a.txt"), "touch\n").unwrap();
    fs::write(t.0.join("u.txt"), "u").unwrap();
    // Make the stat info stale so a plain `git status` would want to refresh the index.
    let before_index = fs::read(t.0.join(".git/index")).unwrap();
    let before_files = (
        fs::read(t.0.join("a.txt")).unwrap(),
        fs::read(t.0.join("u.txt")).unwrap(),
    );
    for _ in 0..3 {
        inspect(&t.0).unwrap();
        tracked_patch(&t.0).unwrap();
    }
    assert_eq!(fs::read(t.0.join(".git/index")).unwrap(), before_index);
    assert_eq!(
        (
            fs::read(t.0.join("a.txt")).unwrap(),
            fs::read(t.0.join("u.txt")).unwrap()
        ),
        before_files
    );
}

#[test]
fn symlinks_are_not_followed() {
    let t = repo("symlink");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/hostname", t.0.join("link")).unwrap();
        let s = inspect(&t.0).unwrap();
        let l = s.untracked.iter().find(|f| f.path == "link").unwrap();
        assert!(l.is_symlink && l.sha256.is_none());
    }
}
