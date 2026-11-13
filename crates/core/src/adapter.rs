use crate::agent::AgentId;
use crate::session::Session;
use std::fmt;
use std::path::PathBuf;

/// Values must reflect implemented and tested behaviour, not assumptions (spec 7.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdapterCapabilities {
    pub list_sessions: bool,
    pub read_session: bool,
    pub read_tool_events: bool,
    pub read_attachments: bool,
    pub native_resume: bool,
    pub native_import: bool,
    pub launch_agent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInstallation {
    pub agent: AgentId,
    pub available: bool,
    pub data_path: Option<PathBuf>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub project_root: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub agent: AgentId,
    pub title: Option<String>,
    pub project_root: Option<String>,
    pub updated_at: Option<String>,
    pub event_count: u64,
}

#[derive(Debug)]
pub enum AdapterError {
    /// Agent data store missing or unreadable.
    Unavailable(String),
    SessionNotFound(String),
    /// The store is readable but its schema is not one we understand.
    UnsupportedFormat(String),
    Source(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "agent data unavailable: {m}"),
            Self::SessionNotFound(id) => write!(f, "session not found: {id}"),
            Self::UnsupportedFormat(m) => write!(f, "unsupported format: {m}"),
            Self::Source(m) => write!(f, "source error: {m}"),
        }
    }
}

impl std::error::Error for AdapterError {}

pub type AdapterResult<T> = Result<T, AdapterError>;

/// Adapters only discover and normalise; they must never write to agent data.
/// Synchronous for now: all current sources are local files.
pub trait AgentAdapter {
    fn id(&self) -> AgentId;
    fn detect(&self) -> AdapterResult<AgentInstallation>;
    fn list_sessions(&self, filter: &SessionFilter) -> AdapterResult<Vec<SessionSummary>>;
    fn read_session(&self, session_id: &str) -> AdapterResult<Session>;
    fn capabilities(&self) -> AdapterCapabilities;
}
