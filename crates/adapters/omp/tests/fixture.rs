use std::path::PathBuf;
use xm_relay_adapter_omp::OmpAdapter;
use xm_relay_core::{AgentAdapter, EventKind, ParseStatus, SessionFilter, ToolStatus};

#[test]
fn user_message_without_text_payload_is_lossy() {
    let dir = std::env::temp_dir().join(format!("xmr-omp-drift-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sub = dir.join("-work-demo");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("2023-11-14T22-00-00-000Z_dddd3333-0000-0000-0000-000000000004.jsonl"),
        "{\"type\":\"session\",\"version\":3,\"id\":\"dddd3333-0000-0000-0000-000000000004\",\"timestamp\":\"2023-11-14T22:00:00.000Z\",\"cwd\":\"/work/drift\"}\n\
         {\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2023-11-14T22:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\"}]}}\n",
    )
    .unwrap();

    let a = OmpAdapter::new(dir.clone());
    let s = a
        .read_session("dddd3333-0000-0000-0000-000000000004")
        .unwrap();
    assert!(
        s.parse_report.is_lossy(),
        "schema drift must mark the report lossy"
    );
    let ev = s
        .events
        .iter()
        .find(|e| e.parse_status == ParseStatus::Partial)
        .expect("drifted user message should be Partial");
    assert_eq!(ev.kind, EventKind::UserMessage);
    assert!(ev.content.text.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

fn adapter() -> OmpAdapter {
    OmpAdapter::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/omp"))
}

const A: &str = "aaaa1111-0000-0000-0000-000000000001";
const B: &str = "bbbb2222-0000-0000-0000-000000000002";

#[test]
fn detects_and_lists_newest_first() {
    let a = adapter();
    assert!(a.detect().unwrap().available);
    let l = a.list_sessions(&SessionFilter::default()).unwrap();
    assert_eq!(l.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [B, A]);
    let first = l.iter().find(|s| s.id == A).unwrap();
    assert_eq!(first.title.as_deref(), Some("Add login endpoint"));
    assert_eq!(first.project_root.as_deref(), Some("/work/demo"));
}

#[test]
fn normalises_session_without_assuming_success() {
    let s = adapter().read_session(A).unwrap();
    let results: Vec<_> = s
        .events
        .iter()
        .filter(|e| e.kind == EventKind::ToolResult)
        .map(|e| (e.tool.as_ref().unwrap().name.as_str(), e.status))
        .collect();
    assert_eq!(
        results,
        [
            ("write", ToolStatus::Succeeded),
            ("bash", ToolStatus::Failed),
            ("edit", ToolStatus::Failed),
            ("bash", ToolStatus::Succeeded)
        ]
    );
    // call input attached to the result
    let w = s
        .events
        .iter()
        .find(|e| e.kind == EventKind::ToolResult)
        .unwrap();
    assert_eq!(
        w.tool.as_ref().unwrap().input.as_ref().unwrap()["path"],
        "src/login.rs"
    );
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == EventKind::FileChange)
            .count(),
        1
    );
    // the read call (c4) has no result: it stays a ToolCall with unknown status
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == EventKind::ToolCall)
            .count(),
        4
    );
    let comp = s
        .events
        .iter()
        .find(|e| e.kind == EventKind::Compaction)
        .unwrap();
    assert_eq!(
        comp.content.text.as_deref(),
        Some("Login endpoint written; tests failing.")
    );
    assert!(s
        .events
        .iter()
        .any(|e| e.kind == EventKind::Error
            && e.content.text.as_deref() == Some("rate limit reached")));
    assert!(!s
        .events
        .iter()
        .any(|e| e.content.text.as_deref() == Some("hidden")));
    assert!(!s.parse_report.is_lossy());
    assert_eq!(s.source.format_version.as_deref(), Some("3"));
    assert_eq!(s.started_at.as_deref(), Some("2023-11-14T22:13:20.000Z"));
}

#[test]
fn reports_unsupported_and_invalid_lines() {
    let s = adapter().read_session(B).unwrap();
    let u: Vec<_> = s
        .parse_report
        .unsupported
        .iter()
        .map(|u| (u.position, u.native_kind.clone().unwrap()))
        .collect();
    assert_eq!(
        u,
        [
            (3, "hologram".to_string()),
            (4, "message:alien".to_string()),
            (5, "mystery".to_string())
        ]
    );
    assert_eq!(s.parse_report.errors.len(), 1);
    assert!(s.parse_report.is_lossy());
}

#[test]
fn missing_session_and_dir() {
    assert!(adapter().read_session("nope").is_err());
    let m = OmpAdapter::new("/nonexistent/omp");
    assert!(!m.detect().unwrap().available);
    assert!(m.list_sessions(&SessionFilter::default()).is_err());
}

#[test]
fn ignores_virtual_uri_paths_for_file_change() {
    let dir = std::env::temp_dir().join(format!("xmr-omp-uri-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let jsonl = [
        r#"{"type":"session","version":3,"id":"uri-sess","timestamp":"2026-11-15T14:36:43.676Z","cwd":"/tmp/proj"}"#,
        r#"{"type":"message","id":"m1","timestamp":"2026-11-15T14:36:44Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"write","arguments":{"path":"xd://mcp__dart_dtd","content":"x"}},{"type":"toolCall","id":"c2","name":"edit","arguments":{"path":"lib/main.dart","oldText":"a","newText":"b"}}]}}"#,
        r#"{"type":"message","id":"r1","timestamp":"2026-11-15T14:36:45Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"write","isError":false,"content":[{"type":"text","text":"ok"}]}}"#,
        r#"{"type":"message","id":"r2","timestamp":"2026-11-15T14:36:46Z","message":{"role":"toolResult","toolCallId":"c2","toolName":"edit","isError":false,"content":[{"type":"text","text":"ok"}]}}"#,
    ]
    .join("\n");
    std::fs::write(proj.join("2026-11-15T14-36-43-676Z_uri-sess.jsonl"), jsonl).unwrap();

    let s = OmpAdapter::new(&dir).read_session("uri-sess").unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let changes: Vec<_> = s
        .events
        .iter()
        .filter(|e| e.kind == EventKind::FileChange)
        .filter_map(|e| e.content.structured.as_ref()?.get("path")?.as_str())
        .collect();
    assert_eq!(changes, ["lib/main.dart"]);
}
