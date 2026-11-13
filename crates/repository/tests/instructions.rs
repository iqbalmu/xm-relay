use std::fs;
use std::path::PathBuf;
use std::process::Command;
use xm_relay_repository::{discover_instructions, sha256_hex, InstructionKind};

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("xmr-ins-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    fs::canonicalize(&p).unwrap()
}

#[test]
fn finds_root_and_nested_files_and_marks_applicability() {
    let r = tmp("nested");
    Command::new("git")
        .arg("-C")
        .arg(&r)
        .args(["init", "-q"])
        .status()
        .unwrap();
    fs::write(r.join("AGENTS.md"), "root rules").unwrap();
    fs::write(r.join("CLAUDE.md"), "claude rules").unwrap();
    fs::create_dir_all(r.join("pkg/api")).unwrap();
    fs::write(r.join("pkg/AGENTS.md"), "pkg rules").unwrap();
    fs::create_dir_all(r.join("other")).unwrap();
    fs::write(r.join("other/AGENTS.md"), "other rules").unwrap();
    fs::create_dir_all(r.join(".github")).unwrap();
    fs::write(r.join(".github/copilot-instructions.md"), "gh").unwrap();
    fs::create_dir_all(r.join("node_modules/x")).unwrap();
    fs::write(r.join("node_modules/x/AGENTS.md"), "ignored").unwrap();

    let s = discover_instructions(&r.join("pkg/api"));
    let by: Vec<_> = s
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.applies_to_start))
        .collect();
    assert!(by.contains(&("AGENTS.md", true)));
    assert!(by.contains(&("CLAUDE.md", true)));
    assert!(by.contains(&("pkg/AGENTS.md", true)));
    assert!(by.contains(&("other/AGENTS.md", false)));
    assert!(by.contains(&(".github/copilot-instructions.md", true)));
    assert!(!s.files.iter().any(|f| f.path.contains("node_modules")));
    assert_eq!(s.root, r.display().to_string());
    let a = s.files.iter().find(|f| f.path == "AGENTS.md").unwrap();
    assert_eq!(a.kind, InstructionKind::Agents);
    assert_eq!(a.content.as_deref(), Some("root rules"));
    assert_eq!(
        a.sha256.as_deref(),
        Some(sha256_hex(b"root rules").as_str())
    );
    // root-scope files come first
    assert_eq!(s.files[0].scope, "");
    fs::remove_dir_all(&r).unwrap();
}

#[test]
fn symlinks_truncation_and_binary_are_handled_safely() {
    let r = tmp("edge");
    fs::write(r.join("secret.txt"), "TOP SECRET").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(r.join("secret.txt"), r.join("AGENTS.md")).unwrap();
    fs::write(r.join("GEMINI.md"), vec![b'a'; 300 * 1024]).unwrap();
    fs::write(r.join("CLAUDE.md"), [0xff, 0xfe, 0x00]).unwrap();
    let s = discover_instructions(&r);
    #[cfg(unix)]
    {
        let l = s.files.iter().find(|f| f.path == "AGENTS.md").unwrap();
        assert!(l.is_symlink && l.content.is_none() && l.sha256.is_none());
    }
    let g = s.files.iter().find(|f| f.path == "GEMINI.md").unwrap();
    assert!(g.truncated);
    assert_eq!(g.content.as_ref().unwrap().len(), 256 * 1024);
    assert_eq!(g.size, 300 * 1024);
    let c = s.files.iter().find(|f| f.path == "CLAUDE.md").unwrap();
    assert!(c.content.is_none());
    assert!(s
        .warnings
        .iter()
        .any(|w| w.contains("symlink") || cfg!(not(unix))));
    assert!(
        s.warnings.iter().any(|w| w.contains("truncated"))
            && s.warnings.iter().any(|w| w.contains("UTF-8"))
    );
    fs::remove_dir_all(&r).unwrap();
}

#[test]
fn empty_project_yields_empty_set() {
    let r = tmp("empty");
    let s = discover_instructions(&r);
    assert!(s.files.is_empty() && s.warnings.is_empty());
    fs::remove_dir_all(&r).unwrap();
}
