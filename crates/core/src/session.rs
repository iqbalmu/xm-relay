use crate::agent::AgentInfo;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    UserMessage,
    AssistantMessage,
    ToolCall,
    ToolResult,
    FileChange,
    Checkpoint,
    Error,
    Compaction,
    Attachment,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    User,
    Assistant,
    System,
    Tool,
    Unknown,
}

/// Spec 8.2: never infer `Succeeded` merely because a tool call was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Succeeded,
    Failed,
    Cancelled,
    #[default]
    Unknown,
    NotRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseStatus {
    Parsed,
    /// Recognised but partially understood.
    Partial,
    #[default]
    Unsupported,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventContent {
    pub text: Option<String>,
    pub structured: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub input: Option<serde_json::Value>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSourceRef {
    pub session_id: String,
    pub native_event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub sequence: u64,
    /// RFC 3339 with explicit timezone, when known.
    pub timestamp: Option<String>,
    pub kind: EventKind,
    pub actor: Actor,
    pub content: EventContent,
    pub tool: Option<ToolInfo>,
    pub status: ToolStatus,
    pub source_ref: EventSourceRef,
    pub parse_status: ParseStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSource {
    pub path: String,
    pub format: String,
    pub format_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRef {
    pub root: String,
    pub repository: bool,
}

/// One unrecognised event; position is kept so it is never silently dropped (spec 4.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedEvent {
    pub position: u64,
    pub native_kind: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseReport {
    pub total_events: u64,
    pub parsed_events: u64,
    pub unsupported: Vec<UnsupportedEvent>,
    pub errors: Vec<String>,
}

impl ParseReport {
    pub fn unsupported_events(&self) -> u64 {
        self.unsupported.len() as u64
    }

    /// True when anything was skipped or failed; handoff confidence must drop.
    pub fn is_lossy(&self) -> bool {
        !self.unsupported.is_empty() || !self.errors.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub agent: AgentInfo,
    pub source: SessionSource,
    pub project: Option<ProjectRef>,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub events: Vec<Event>,
    pub parse_report: ParseReport,
}
