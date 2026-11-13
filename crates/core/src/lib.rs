//! Vendor-neutral domain model for xm-relay. Must not depend on any adapter.

pub mod agent;
pub mod evidence;
pub mod session;
pub mod task;

pub use agent::{AgentId, AgentInfo};
pub use evidence::{
    Classification, Confidence, Evidence, EvidenceItem, SourceRef, SourceType, Verification,
};
pub use session::{
    Actor, Event, EventContent, EventKind, ParseReport, ParseStatus, Session, ToolInfo, ToolStatus,
    UnsupportedEvent,
};
pub use task::{Task, TaskState, TaskStatus};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ids_are_stable() {
        let ids: Vec<_> = AgentId::ALL.iter().map(|a| a.as_str()).collect();
        assert_eq!(ids, ["opencode", "antigravity-cli", "omp"]);
        assert_eq!(
            serde_json::to_string(&AgentId::AntigravityCli).unwrap(),
            "\"antigravity-cli\""
        );
    }

    #[test]
    fn tool_status_defaults_to_unknown() {
        assert_eq!(ToolStatus::default(), ToolStatus::Unknown);
    }

    #[test]
    fn reported_evidence_is_never_verified() {
        let e = EvidenceItem::reported("E-1", "tests pass", SourceRef::default());
        assert_eq!(e.classification, Classification::Reported);
        assert_eq!(e.verification.result, "unknown");
    }

    #[test]
    fn completed_task_without_evidence_is_flagged() {
        let ts = TaskState {
            objective: "x".into(),
            tasks: vec![
                Task {
                    id: "T1".into(),
                    title: "a".into(),
                    status: TaskStatus::Completed,
                    confidence: Confidence::Low,
                    evidence_refs: vec![],
                },
                Task {
                    id: "T2".into(),
                    title: "b".into(),
                    status: TaskStatus::Completed,
                    confidence: Confidence::High,
                    evidence_refs: vec!["E-1".into()],
                },
                Task {
                    id: "T3".into(),
                    title: "c".into(),
                    status: TaskStatus::Pending,
                    confidence: Confidence::High,
                    evidence_refs: vec![],
                },
            ],
        };
        assert_eq!(ts.unsupported_completions(), ["T1"]);
    }

    #[test]
    fn confidence_orders_low_to_high() {
        assert!(Confidence::High > Confidence::Medium && Confidence::Low > Confidence::Unknown);
    }

    #[test]
    fn parse_report_lossy_when_unsupported() {
        let mut r = ParseReport::default();
        assert!(!r.is_lossy());
        r.unsupported.push(UnsupportedEvent {
            position: 7,
            native_kind: Some("x".into()),
            reason: "unknown".into(),
        });
        assert!(r.is_lossy());
        assert_eq!(r.unsupported_events(), 1);
    }

    #[test]
    fn session_roundtrips_json() {
        let s = Session {
            id: "s1".into(),
            agent: AgentInfo {
                id: AgentId::Omp,
                version: None,
                adapter_version: "0.1.0".into(),
            },
            source: session::SessionSource {
                path: "/p".into(),
                format: "jsonl".into(),
                format_version: None,
            },
            project: None,
            started_at: None,
            updated_at: None,
            events: vec![Event {
                id: "e1".into(),
                sequence: 1,
                timestamp: None,
                kind: EventKind::ToolResult,
                actor: Actor::Assistant,
                content: EventContent::default(),
                tool: None,
                status: ToolStatus::Unknown,
                source_ref: session::EventSourceRef {
                    session_id: "s1".into(),
                    native_event_id: None,
                },
                parse_status: ParseStatus::Parsed,
            }],
            parse_report: ParseReport::default(),
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&j).unwrap(), s);
    }
}
