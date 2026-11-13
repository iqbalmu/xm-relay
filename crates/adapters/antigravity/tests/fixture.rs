use rusqlite::Connection;
use std::path::PathBuf;
use xm_relay_adapter_antigravity::AntigravityAdapter;
use xm_relay_core::{AgentAdapter, EventKind, ParseStatus, SessionFilter, ToolStatus};

const A: &str = "aaaa1111-0000-0000-0000-000000000001";
const B: &str = "bbbb2222-0000-0000-0000-000000000002";

fn adapter(tag: &str) -> (PathBuf, AntigravityAdapter) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/antigravity");
    let db = std::env::temp_dir().join(format!("xmr-agy-{}-{tag}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let sql = std::fs::read_to_string(root.join("conversation_summaries.sql")).unwrap();
    Connection::open(&db).unwrap().execute_batch(&sql).unwrap();
    (
        db.clone(),
        AntigravityAdapter::new(root).with_summaries_db(db),
    )
}

#[test]
fn lists_only_readable_conversations() {
    let (db, a) = adapter("list");
    assert!(a.detect().unwrap().available);
    let l = a.list_sessions(&SessionFilter::default()).unwrap();
    // cccc... has no transcript, so it is not listed
    assert_eq!(l.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [B, A]);
    assert_eq!(l[1].title.as_deref(), Some("Add login endpoint"));
    assert_eq!(l[1].project_root.as_deref(), Some("/work/demo"));
    assert_eq!(l[1].updated_at.as_deref(), Some("2023-11-14T22:13:30.123Z"));
    let _ = std::fs::remove_file(db);
}

#[test]
fn normalises_tools_without_assuming_success() {
    let (db, a) = adapter("read");
    let s = a.read_session(A).unwrap();
    let results: Vec<_> = s
        .events
        .iter()
        .filter(|e| e.kind == EventKind::ToolResult)
        .map(|e| (e.tool.as_ref().unwrap().name.as_str(), e.status))
        .collect();
    assert_eq!(
        results,
        [
            ("write_file", ToolStatus::Succeeded),
            ("run_command", ToolStatus::Failed),
            ("edit_file", ToolStatus::Failed),
            ("run_command", ToolStatus::Unknown)
        ]
    );
    // read_file never got a result: still a ToolCall only
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == EventKind::ToolCall)
            .count(),
        5
    );
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == EventKind::FileChange)
            .count(),
        1
    );
    assert!(s.events.iter().any(|e| e.kind == EventKind::Checkpoint
        && e.content.text.as_deref() == Some("Summary of earlier work.")));
    assert!(
        s.events
            .iter()
            .any(|e| e.kind == EventKind::Error
                && e.content.text.as_deref() == Some("quota exceeded"))
    );
    assert!(!s
        .events
        .iter()
        .any(|e| e.content.text.as_deref() == Some("hidden")));
    assert!(!s.parse_report.is_lossy());
    assert_eq!(s.project.as_ref().unwrap().root, "/work/demo");
    assert_eq!(s.started_at.as_deref(), Some("2023-11-14T22:13:20Z"));
    let _ = std::fs::remove_file(db);
}

#[test]
fn reports_unsupported_orphans_truncation_and_bad_json() {
    let (db, a) = adapter("weird");
    let s = a.read_session(B).unwrap();
    let u: Vec<_> = s
        .parse_report
        .unsupported
        .iter()
        .map(|u| (u.position, u.native_kind.clone().unwrap()))
        .collect();
    assert_eq!(u, [(1, "HOLOGRAM".to_string())]);
    assert!(s.parse_report.is_lossy());
    let errs = s.parse_report.errors.join("|");
    assert!(
        errs.contains("without a pending tool call")
            && errs.contains("truncated")
            && errs.contains("invalid JSON")
    );
    // the truncated assistant message is marked partial
    assert!(s
        .events
        .iter()
        .any(|e| e.kind == EventKind::AssistantMessage && e.parse_status == ParseStatus::Partial));
    let _ = std::fs::remove_file(db);
}

#[test]
fn rejects_unknown_and_traversing_ids() {
    let (db, a) = adapter("ids");
    assert!(a.read_session("nope").is_err());
    assert!(a.read_session("../x").is_err());
    assert!(a.read_session("").is_err());
    let _ = std::fs::remove_file(db);
}
