//! Read-only Antigravity CLI (`agy`) adapter.
//!
//! Sources, under the agent data dir (`~/.gemini/antigravity-cli`):
//! - `brain/<id>/.system_generated/logs/transcript_full.jsonl` (falls back to
//!   `transcript.jsonl`, whose long fields may be truncated): the events.
//! - `conversation_summaries.db` (SQLite): title, workspace and timestamps.
//!
//! The per-conversation `conversations/<id>.db` holds opaque blobs and is not read.
//! Conversations without a transcript cannot be read and are not listed.

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use xm_relay_core::session::{EventSourceRef, ProjectRef, SessionSource, UnsupportedEvent};
use xm_relay_core::{
    Actor, AdapterCapabilities, AdapterError, AdapterResult, AgentAdapter, AgentId, AgentInfo,
    AgentInstallation, Event, EventContent, EventKind, ParseReport, ParseStatus, Session,
    SessionFilter, SessionSummary, ToolInfo, ToolStatus,
};

pub const ADAPTER_VERSION: &str = "0.1.0";
const FORMAT: &str = "antigravity-transcript-jsonl";

pub struct AntigravityAdapter {
    root: PathBuf,
    summaries_db: PathBuf,
}

impl AntigravityAdapter {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let summaries_db = root.join("conversation_summaries.db");
        Self { root, summaries_db }
    }

    /// Override the summaries database location (used by tests).
    pub fn with_summaries_db(mut self, path: impl Into<PathBuf>) -> Self {
        self.summaries_db = path.into();
        self
    }

    /// `~/.gemini/antigravity-cli`.
    pub fn default_root() -> Option<PathBuf> {
        Some(Path::new(&std::env::var_os("HOME")?).join(".gemini/antigravity-cli"))
    }

    fn logs_dir(&self, id: &str) -> PathBuf {
        self.root
            .join("brain")
            .join(id)
            .join(".system_generated/logs")
    }

    fn transcript(&self, id: &str) -> Option<PathBuf> {
        // Ids come from the CLI/db; refuse anything that could escape `brain/`.
        if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
            return None;
        }
        let dir = self.logs_dir(id);
        ["transcript_full.jsonl", "transcript.jsonl"]
            .iter()
            .map(|n| dir.join(n))
            .find(|p| p.is_file())
    }
}

fn src_err(e: impl std::fmt::Display) -> AdapterError {
    AdapterError::Source(e.to_string())
}

/// `2026-11-13 04:46:54.736984721+00:00` -> `2026-11-13T04:46:54.736Z`.
pub fn normalize_timestamp(s: &str) -> String {
    let s = s.replacen(' ', "T", 1);
    let tz_at = s
        .rfind(['+', 'Z'])
        .or_else(|| s.rfind('-').filter(|&i| i > 10));
    let (body, tz) = match tz_at {
        Some(i) => (&s[..i], &s[i..]),
        None => (s.as_str(), ""),
    };
    let body = match body.split_once('.') {
        Some((head, frac)) => format!("{head}.{:0<3}", &frac[..frac.len().min(3)]),
        None => body.to_string(),
    };
    let tz = if tz == "+00:00" { "Z" } else { tz };
    format!("{body}{tz}")
}

fn file_uri_to_path(u: &str) -> String {
    u.strip_prefix("file://").unwrap_or(u).to_string()
}

impl AgentAdapter for AntigravityAdapter {
    fn id(&self) -> AgentId {
        AgentId::AntigravityCli
    }

    fn detect(&self) -> AdapterResult<AgentInstallation> {
        let available = self.root.join("brain").is_dir();
        Ok(AgentInstallation {
            agent: AgentId::AntigravityCli,
            available,
            data_path: available.then(|| self.root.clone()),
            version: None,
        })
    }

    fn list_sessions(&self, filter: &SessionFilter) -> AdapterResult<Vec<SessionSummary>> {
        if !self.root.join("brain").is_dir() {
            return Err(AdapterError::Unavailable(format!(
                "{} not found",
                self.root.display()
            )));
        }
        let conn =
            Connection::open_with_flags(&self.summaries_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| {
                    AdapterError::Unavailable(format!("{}: {e}", self.summaries_db.display()))
                })?;
        let mut stmt = conn
            .prepare("SELECT conversation_id, title, step_count, last_modified_time, workspace_uris FROM conversation_summaries ORDER BY last_modified_time DESC")
            .map_err(|e| AdapterError::UnsupportedFormat(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(src_err)?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title, steps, modified, uris) = row.map_err(src_err)?;
            if self.transcript(&id).is_none() {
                continue;
            }
            let root = serde_json::from_str::<Vec<String>>(&uris)
                .ok()
                .and_then(|v| v.into_iter().next())
                .map(|u| file_uri_to_path(&u));
            if filter
                .project_root
                .as_ref()
                .is_some_and(|p| root.as_ref() != Some(p))
            {
                continue;
            }
            out.push(SessionSummary {
                id,
                agent: AgentId::AntigravityCli,
                title: (!title.is_empty()).then_some(title),
                project_root: root,
                updated_at: Some(normalize_timestamp(&modified)),
                event_count: steps as u64,
            });
            if filter.limit.is_some_and(|l| out.len() >= l) {
                break;
            }
        }
        Ok(out)
    }

    fn read_session(&self, session_id: &str) -> AdapterResult<Session> {
        let path = self
            .transcript(session_id)
            .ok_or_else(|| AdapterError::SessionNotFound(session_id.into()))?;
        let f = File::open(&path).map_err(src_err)?;
        let mut b = Builder {
            session_id: session_id.into(),
            events: Vec::new(),
            report: ParseReport::default(),
            pending: VecDeque::new(),
            last: None,
        };
        let (mut first, mut last) = (None, None);
        for (i, line) in BufReader::new(f).lines().enumerate() {
            let line = line.map_err(src_err)?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    b.report.total_events += 1;
                    b.report
                        .errors
                        .push(format!("line {}: invalid JSON: {e}", i + 1));
                    continue;
                }
            };
            if let Some(t) = v["created_at"].as_str() {
                first.get_or_insert_with(|| t.to_string());
                last = Some(t.to_string());
            }
            b.step(&v);
        }

        let (title_root, _) = self.summary_workspace(session_id);
        Ok(Session {
            id: session_id.into(),
            agent: AgentInfo {
                id: AgentId::AntigravityCli,
                version: None,
                adapter_version: ADAPTER_VERSION.into(),
            },
            source: SessionSource {
                path: path.display().to_string(),
                format: FORMAT.into(),
                format_version: None,
            },
            project: title_root.map(|root| ProjectRef {
                repository: Path::new(&root).join(".git").exists(),
                root,
            }),
            started_at: first,
            updated_at: last,
            events: b.events,
            parse_report: b.report,
        })
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            list_sessions: true,
            read_session: true,
            read_tool_events: true,
            ..Default::default()
        }
    }
}

impl AntigravityAdapter {
    /// Workspace root from the summaries db; `None` when unavailable (never guessed).
    fn summary_workspace(&self, id: &str) -> (Option<String>, Option<String>) {
        let Ok(conn) =
            Connection::open_with_flags(&self.summaries_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        else {
            return (None, None);
        };
        let row: Option<(String, String)> = conn
            .query_row("SELECT workspace_uris, title FROM conversation_summaries WHERE conversation_id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        let Some((uris, title)) = row else {
            return (None, None);
        };
        let root = serde_json::from_str::<Vec<String>>(&uris)
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|u| file_uri_to_path(&u));
        (root, (!title.is_empty()).then_some(title))
    }
}

struct Builder {
    session_id: String,
    events: Vec<Event>,
    report: ParseReport,
    /// Calls of the latest planner step awaiting a result. Results arrive as `GENERIC` steps in call order.
    pending: VecDeque<(String, Value)>,
    /// Last call that received a result; a trailing `RUNNING` step is a progress update for it.
    last: Option<(String, Value)>,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        native: &str,
        ts: Option<&str>,
        kind: EventKind,
        actor: Actor,
        content: EventContent,
        tool: Option<ToolInfo>,
        status: ToolStatus,
        parse: ParseStatus,
    ) {
        let sequence = self.events.len() as u64 + 1;
        self.events.push(Event {
            id: format!("{}:{}", self.session_id, sequence),
            sequence,
            timestamp: ts.map(String::from),
            kind,
            actor,
            content,
            tool,
            status,
            source_ref: EventSourceRef {
                session_id: self.session_id.clone(),
                native_event_id: Some(native.into()),
            },
            parse_status: parse,
        });
        self.report.parsed_events += 1;
    }

    fn text(v: &Value) -> EventContent {
        EventContent {
            text: v["content"].as_str().map(String::from),
            structured: None,
        }
    }

    fn step(&mut self, v: &Value) {
        let index = v["step_index"].as_u64().unwrap_or(0);
        let native = index.to_string();
        let ts = v["created_at"].as_str();
        let truncated = v["truncated_fields"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        let before = self.events.len();

        match v["type"].as_str().unwrap_or("") {
            "USER_INPUT" => {
                self.report.total_events += 1;
                let media = v["media"].as_array().map_or(0, Vec::len);
                let mut c = Self::text(v);
                c.structured = (media > 0).then(|| json!({ "attachment_count": media }));
                self.push(
                    &native,
                    ts,
                    EventKind::UserMessage,
                    Actor::User,
                    c,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Parsed,
                );
            }
            "PLANNER_RESPONSE" => {
                // Results follow their planner step; calls left unanswered must not pair
                // with later results, so drift cannot silently mismatch them.
                self.pending.clear();
                // `thinking` is hidden reasoning and is deliberately not transferred.
                if v["content"].as_str().is_some_and(|s| !s.is_empty()) {
                    self.report.total_events += 1;
                    self.push(
                        &native,
                        ts,
                        EventKind::AssistantMessage,
                        Actor::Assistant,
                        Self::text(v),
                        None,
                        ToolStatus::Unknown,
                        ParseStatus::Parsed,
                    );
                }
                for call in v["tool_calls"].as_array().into_iter().flatten() {
                    self.report.total_events += 1;
                    let name = call["name"].as_str().unwrap_or("unknown").to_string();
                    self.pending.push_back((name.clone(), call["args"].clone()));
                    let tool = ToolInfo {
                        name,
                        input: Some(call["args"].clone()),
                        output: None,
                    };
                    self.push(
                        &native,
                        ts,
                        EventKind::ToolCall,
                        Actor::Assistant,
                        EventContent::default(),
                        Some(tool),
                        ToolStatus::Unknown,
                        ParseStatus::Parsed,
                    );
                }
            }
            "GENERIC" => self.tool_result(index, &native, ts, v),
            "CHECKPOINT" => {
                self.report.total_events += 1;
                self.push(
                    &native,
                    ts,
                    EventKind::Checkpoint,
                    Actor::System,
                    Self::text(v),
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Parsed,
                );
            }
            "ERROR_MESSAGE" => {
                self.report.total_events += 1;
                let c = EventContent {
                    text: v["content"]
                        .as_str()
                        .or(v["error"].as_str())
                        .map(String::from),
                    structured: None,
                };
                self.push(
                    &native,
                    ts,
                    EventKind::Error,
                    Actor::System,
                    c,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Parsed,
                );
            }
            "SYSTEM_MESSAGE" => {
                self.report.total_events += 1;
                self.push(
                    &native,
                    ts,
                    EventKind::Unknown,
                    Actor::System,
                    Self::text(v),
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Partial,
                );
            }
            other => {
                self.report.total_events += 1;
                self.report.unsupported.push(UnsupportedEvent {
                    position: index,
                    native_kind: Some(other.into()),
                    reason: "unrecognised step type".into(),
                });
            }
        }

        if truncated {
            for e in &mut self.events[before..] {
                e.parse_status = ParseStatus::Partial;
            }
            self.report.errors.push(format!(
                "step {index}: fields truncated in source ({})",
                v["truncated_fields"]
            ));
        }
    }

    fn tool_result(&mut self, index: u64, native: &str, ts: Option<&str>, v: &Value) {
        self.report.total_events += 1;
        let progress = v["status"] == "RUNNING" && self.pending.is_empty();
        let (name, input, matched) = match self.pending.pop_front() {
            Some(c) => {
                self.last = Some(c.clone());
                (c.0, Some(c.1), true)
            }
            None => match (&self.last, progress) {
                (Some((n, a)), true) => (n.clone(), Some(a.clone()), true),
                _ => ("unknown".to_string(), None, false),
            },
        };
        let content = v["content"].as_str().unwrap_or("");
        // A recorded step is not a success: use explicit signals only.
        let status = match v["status"].as_str() {
            Some("ERROR") | Some("INVALID") => ToolStatus::Failed,
            Some("DONE") if name == "run_command" => match exit_code(content) {
                Some(0) => ToolStatus::Succeeded,
                Some(_) => ToolStatus::Failed,
                None => ToolStatus::Unknown,
            },
            Some("DONE") => ToolStatus::Succeeded,
            _ => ToolStatus::Unknown,
        };
        let parse = if matched {
            ParseStatus::Parsed
        } else {
            ParseStatus::Partial
        };
        if !matched {
            self.report.errors.push(format!(
                "step {index}: tool result without a pending tool call"
            ));
        }
        let tool = ToolInfo {
            name: name.clone(),
            input: input.clone(),
            output: (!content.is_empty()).then(|| content.to_string()),
        };
        self.push(
            native,
            ts,
            EventKind::ToolResult,
            Actor::Tool,
            EventContent::default(),
            Some(tool),
            status,
            parse,
        );

        if matches!(name.as_str(), "edit_file" | "write_file") && status == ToolStatus::Succeeded {
            if let Some(path) = input.as_ref().and_then(|i| i["Path"].as_str()) {
                self.report.total_events += 1;
                let c = EventContent {
                    text: None,
                    structured: Some(json!({ "path": path, "tool": name })),
                };
                self.push(
                    native,
                    ts,
                    EventKind::FileChange,
                    Actor::Tool,
                    c,
                    None,
                    ToolStatus::Succeeded,
                    ParseStatus::Parsed,
                );
            }
        }
    }
}

/// Exit code from a `Command exited with code N` line in `run_command` output.
fn exit_code(output: &str) -> Option<i32> {
    let rest = output.split("Command exited with code ").nth(1)?;
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_timestamps() {
        assert_eq!(
            normalize_timestamp("2026-11-13 04:46:54.736984721+00:00"),
            "2026-11-13T04:46:54.736Z"
        );
        assert_eq!(
            normalize_timestamp("2023-11-14 23:00:05+00:00"),
            "2023-11-14T23:00:05Z"
        );
        assert_eq!(
            normalize_timestamp("2023-11-14 23:00:05.5+07:00"),
            "2023-11-14T23:00:05.500+07:00"
        );
    }

    #[test]
    fn parses_exit_codes() {
        assert_eq!(exit_code("Command exited with code 101\nx"), Some(101));
        assert_eq!(exit_code("Command exited with code 0"), Some(0));
        assert_eq!(exit_code("still running"), None);
    }
}
