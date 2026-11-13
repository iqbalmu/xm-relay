//! Vendor-neutral domain model for xm-relay. Must not depend on any adapter.

/// Agents supported (or planned) by xm-relay, in adapter implementation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentId {
    OpenCode,
    AntigravityCli,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ids_are_stable() {
        let ids: Vec<_> = AgentId::ALL.iter().map(|a| a.as_str()).collect();
        assert_eq!(ids, ["opencode", "antigravity-cli", "omp"]);
    }
}
