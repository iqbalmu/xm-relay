use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use xm_relay_core::session::{EventSourceRef, ProjectRef, SessionSource};
use xm_relay_core::{
    Actor, AgentId, AgentInfo, Classification, Confidence, Event, EventContent, EventKind,
    ParseReport, ParseStatus, Session, ToolInfo, ToolStatus,
};
use xm_relay_handoff::{
    create_handoff, export_handoff, generate_continuation_prompt, list_handoffs, validate_handoff,
    PreflightStatus,
};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("xmr-ho-{}-{tag}", std::process::id()));
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

fn sample_session(root: &Path, written_file: &str, secret: &str) -> Session {
    Session {
        id: "ses_demo".into(),
        agent: AgentInfo {
            id: AgentId::OpenCode,
            version: Some("2.2.6".into()),
            adapter_version: "0.1.0".into(),
        },
        source: SessionSource {
            path: "/tmp/opencode.db".into(),
            format: "opencode-sqlite".into(),
            format_version: Some("session_v2".into()),
        },
        project: Some(ProjectRef {
            root: root.display().to_string(),
            repository: true,
        }),
        started_at: Some("2026-11-13T00:00:00Z".into()),
        updated_at: Some("2026-11-13T00:05:00Z".into()),
        events: vec![
            Event {
                id: "e1".into(),
                sequence: 1,
                timestamp: None,
                kind: EventKind::UserMessage,
                actor: Actor::User,
                content: EventContent {
                    text: Some(format!("Implement auth endpoint using token {secret}")),
                    structured: None,
                },
                tool: None,
                status: ToolStatus::Unknown,
                source_ref: EventSourceRef {
                    session_id: "ses_demo".into(),
                    native_event_id: Some("m1".into()),
                },
                parse_status: ParseStatus::Parsed,
            },
            Event {
                id: "e2".into(),
                sequence: 2,
                timestamp: None,
                kind: EventKind::FileChange,
                actor: Actor::Tool,
                content: EventContent {
                    text: None,
                    structured: Some(json!({ "path": written_file, "tool": "write" })),
                },
                tool: None,
                status: ToolStatus::Succeeded,
                source_ref: EventSourceRef {
                    session_id: "ses_demo".into(),
                    native_event_id: Some("t1".into()),
                },
                parse_status: ParseStatus::Parsed,
            },
            Event {
                id: "e3".into(),
                sequence: 3,
                timestamp: None,
                kind: EventKind::ToolResult,
                actor: Actor::Tool,
                content: EventContent::default(),
                tool: Some(ToolInfo {
                    name: "shell".into(),
                    input: Some(json!({ "command": "cargo test" })),
                    output: Some("1 test failed".into()),
                }),
                status: ToolStatus::Failed,
                source_ref: EventSourceRef {
                    session_id: "ses_demo".into(),
                    native_event_id: Some("t2".into()),
                },
                parse_status: ParseStatus::Parsed,
            },
        ],
        parse_report: ParseReport {
            total_events: 3,
            parsed_events: 3,
            unsupported: Vec::new(),
            errors: Vec::new(),
        },
    }
}

#[test]
fn builds_verifies_redacts_and_detects_missing_file_conflict() {
    let t = Tmp::new("flow");
    let repo = t.0.join("repo");
    let store = t.0.join("store");
    fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("AGENTS.md"), "always run tests\n").unwrap();
    fs::write(repo.join("src/auth.rs"), "pub fn auth() {}\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);

    let secret = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    let sess = sample_session(&repo, "src/auth.rs", &secret);

    let detail = create_handoff(&sess, AgentId::Omp, Some(&repo), &store).unwrap();
    assert_eq!(
        detail.validation.status,
        PreflightStatus::PassedWithWarnings
    );
    assert!(detail.validation.checksums_valid);
    assert_eq!(detail.manifest.confidence.overall, Confidence::Medium);
    assert_eq!(
        detail.evidence.items[0].classification,
        Classification::Verified
    );
    assert!(!detail.context_md.contains(&secret));
    assert!(detail.context_md.contains("[REDACTED:aws_access_key]"));

    let exp_root = t.0.join("exported");
    let exp_path = export_handoff(&store, &detail.manifest.handoff_id, &exp_root, false).unwrap();
    assert!(exp_path.join("manifest.json").is_file());
    assert!(export_handoff(&store, &detail.manifest.handoff_id, &exp_root, false).is_err());
    assert!(export_handoff(&store, &detail.manifest.handoff_id, &exp_root, true).is_ok());

    let list = list_handoffs(&store).unwrap();
    assert_eq!(list.len(), 1);

    let pkg_dir = store.join(&detail.manifest.handoff_id);
    let prompt = generate_continuation_prompt(&detail, &pkg_dir);
    assert!(prompt.contains("Do not assume the handoff summary is fully correct"));

    // Delete the verified file on disk -> live validation blocks continuation!
    fs::remove_file(repo.join("src/auth.rs")).unwrap();
    let reval = validate_handoff(&store, &detail.manifest.handoff_id, Some(&repo)).unwrap();
    assert_eq!(reval.validation.status, PreflightStatus::Blocked);
    assert!(reval.validation.conflicts.iter().any(|c| c.critical));

    // Also test creating a handoff when a claimed file is already missing
    let bad_sess = sample_session(&repo, "src/ghost.rs", &secret);
    let blocked = create_handoff(&bad_sess, AgentId::AntigravityCli, Some(&repo), &store).unwrap();
    assert_eq!(blocked.validation.status, PreflightStatus::Blocked);
    assert_eq!(blocked.manifest.confidence.overall, Confidence::Low);
}
