use rusqlite::Connection;
use xm_relay_adapter_opencode::OpenCodeAdapter;
use xm_relay_core::{AgentAdapter, EventKind, SessionFilter, ToolStatus};

fn fixture_db() -> (tempdir::Dir, OpenCodeAdapter) {
    let sql = include_str!("../../../../fixtures/opencode/basic.sql");
    let dir = tempdir::Dir::new();
    let path = dir.path().join("opencode.db");
    Connection::open(&path).unwrap().execute_batch(sql).unwrap();
    (dir, OpenCodeAdapter::new(path))
}

mod tempdir {
    use std::path::{Path, PathBuf};
    pub struct Dir(PathBuf);
    impl Dir {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(
                format!(
                    "xmr-oc-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                )
                .replace(['(', ')', ' '], ""),
            );
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn detects_and_lists() {
    let (_d, a) = fixture_db();
    assert!(a.detect().unwrap().available);
    assert_eq!(a.detect().unwrap().version.as_deref(), Some("1.2.3"));
    let list = a.list_sessions(&SessionFilter::default()).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "ses_2"); // most recently updated first
    assert_eq!(list[1].event_count, 4);
}

#[test]
fn normalises_tool_statuses_without_assuming_success() {
    let (_d, a) = fixture_db();
    let s = a.read_session("ses_1").unwrap();
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
            ("shell", ToolStatus::Failed),
            ("edit", ToolStatus::Failed),
            ("shell", ToolStatus::Unknown)
        ]
    );
    // Only the successful write yields a FileChange; failed edit does not.
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == EventKind::FileChange)
            .count(),
        1
    );
    // Provider quota error is preserved.
    assert!(
        s.events
            .iter()
            .any(|e| e.kind == EventKind::Error
                && e.content.text.as_deref() == Some("quota exceeded"))
    );
    assert!(!s.parse_report.is_lossy());
    assert_eq!(s.agent.version.as_deref(), Some("1.2.3"));
    assert_eq!(s.started_at.as_deref(), Some("2023-11-14T22:13:20.000Z"));
    // reasoning is not transferred
    assert!(!s
        .events
        .iter()
        .any(|e| e.content.text.as_deref() == Some("hidden")));
}

#[test]
fn reports_unsupported_and_invalid_data() {
    let (_d, a) = fixture_db();
    let s = a.read_session("ses_2").unwrap();
    assert!(s.parse_report.is_lossy());
    let pos: Vec<_> = s
        .parse_report
        .unsupported
        .iter()
        .map(|u| (u.position, u.native_kind.clone().unwrap()))
        .collect();
    assert_eq!(
        pos,
        [(2, "hologram".to_string()), (3, "mystery".to_string())]
    );
    assert_eq!(s.parse_report.errors.len(), 1);
    assert_eq!(s.parse_report.total_events, 4);
    assert_eq!(s.parse_report.parsed_events, 1);
}

#[test]
fn missing_session_and_missing_db() {
    let (_d, a) = fixture_db();
    assert!(a.read_session("nope").is_err());
    let missing = OpenCodeAdapter::new("/nonexistent/opencode.db");
    assert!(!missing.detect().unwrap().available);
    assert!(missing.list_sessions(&SessionFilter::default()).is_err());
}
