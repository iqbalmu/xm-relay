use serde::{Deserialize, Serialize};

/// Spec 4.3: reported/inferred must never be promoted to verified without a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Verified,
    Reported,
    Inferred,
    Unknown,
    Conflict,
}

/// Spec 10.2: qualitative only, no numeric score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Unknown,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Repository,
    Session,
    Tool,
    User,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub method: String,
    /// e.g. "passed", "failed", "unknown"
    pub result: String,
}

impl Verification {
    pub fn not_verified() -> Self {
        Self {
            method: "not_verified".into(),
            result: "unknown".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub id: String,
    pub claim: String,
    pub classification: Classification,
    pub source_type: SourceType,
    pub source_ref: SourceRef,
    pub verification: Verification,
}

impl EvidenceItem {
    /// A claim taken from a session. Always `Reported`/unverified; callers
    /// must run a real check to upgrade it.
    pub fn reported(
        id: impl Into<String>,
        claim: impl Into<String>,
        source_ref: SourceRef,
    ) -> Self {
        Self {
            id: id.into(),
            claim: claim.into(),
            classification: Classification::Reported,
            source_type: SourceType::Session,
            source_ref,
            verification: Verification::not_verified(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub items: Vec<EvidenceItem>,
}
