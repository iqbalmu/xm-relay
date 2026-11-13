use serde::{Deserialize, Serialize};

/// Agents supported (or planned) by xm-relay, in adapter implementation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentId {
    #[serde(rename = "opencode")]
    OpenCode,
    #[serde(rename = "antigravity-cli")]
    AntigravityCli,
    #[serde(rename = "omp")]
    Omp,
}

impl AgentId {
    pub const ALL: [AgentId; 3] = [AgentId::OpenCode, AgentId::AntigravityCli, AgentId::Omp];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentId::OpenCode => "opencode",
            AgentId::AntigravityCli => "antigravity-cli",
            AgentId::Omp => "omp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: AgentId,
    /// `None` when the agent version cannot be detected (never guessed).
    pub version: Option<String>,
    pub adapter_version: String,
}
