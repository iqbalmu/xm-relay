use std::path::PathBuf;
use xm_relay_adapter_omp::OmpAdapter;
use xm_relay_core::{AgentAdapter, EventKind, SessionFilter, ToolStatus};

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
