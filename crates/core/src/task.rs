use crate::evidence::Confidence;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Completed,
    InProgress,
    Pending,
    Blocked,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    pub confidence: Confidence,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskState {
    pub objective: String,
    pub tasks: Vec<Task>,
}

impl TaskState {
    /// Spec 9.4: a task may only be `completed` with evidence backing it.
    /// Returns ids of tasks that violate this rule.
    pub fn unsupported_completions(&self) -> Vec<&str> {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed && t.evidence_refs.is_empty())
            .map(|t| t.id.as_str())
            .collect()
    }
}
