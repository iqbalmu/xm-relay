//! Read-only OpenCode adapter. Source: `opencode.db` (SQLite), tables
//! `session_v2` and `session_message`. Never writes to the database.

mod time;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use xm_relay_core::session::{EventSourceRef, ProjectRef, SessionSource, UnsupportedEvent};
use xm_relay_core::{
    Actor, AdapterCapabilities, AdapterError, AdapterResult, AgentAdapter, AgentId, AgentInfo,
    AgentInstallation, Event, EventContent, EventKind, ParseReport, ParseStatus, Session,
    SessionFilter, SessionSummary, ToolInfo, ToolStatus,
};

pub const ADAPTER_VERSION: &str = "0.1.0";
const FORMAT: &str = "opencode-sqlite";
const FORMAT_VERSION: &str = "session_v2";

pub struct OpenCodeAdapter {
    db_path: PathBuf,
}

impl OpenCodeAdapter {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    /// `$XDG_DATA_HOME/opencode/opencode.db`, falling back to `~/.local/share`.
    pub fn default_db_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))?;
        Some(base.join("opencode/opencode.db"))
    }

    fn open(&self) -> AdapterResult<Connection> {
        if !self.db_path.exists() {
            return Err(AdapterError::Unavailable(format!(
                "{} not found",
                self.db_path.display()
            )));
        }
        let conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        let tables: u32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('session_v2','session_message')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| AdapterError::UnsupportedFormat(e.to_string()))?;
        if tables != 2 {
            return Err(AdapterError::UnsupportedFormat(
                "expected tables session_v2 and session_message".into(),
            ));
        }
        Ok(conn)
    }
}

fn src_err(e: rusqlite::Error) -> AdapterError {
    AdapterError::Source(e.to_string())
}

impl AgentAdapter for OpenCodeAdapter {
    fn id(&self) -> AgentId {
        AgentId::OpenCode
    }

    fn detect(&self) -> AdapterResult<AgentInstallation> {
        let available = self.open().is_ok();
        let version = if available {
            self.open()?
                .query_row(
                    "SELECT version FROM session_v2 ORDER BY time_updated DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .ok()
        } else {
            None
        };
        Ok(AgentInstallation {
            agent: AgentId::OpenCode,
            available,
            data_path: available.then(|| self.db_path.clone()),
            version,
        })
    }

    fn list_sessions(&self, filter: &SessionFilter) -> AdapterResult<Vec<SessionSummary>> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.title, s.directory, s.time_updated,
                        (SELECT count(*) FROM session_message m WHERE m.session_id = s.id)
                 FROM session_v2 s WHERE s.time_archived IS NULL
                 ORDER BY s.time_updated DESC",
            )
            .map_err(src_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(SessionSummary {
                    id: r.get(0)?,
                    agent: AgentId::OpenCode,
                    title: r.get(1)?,
                    project_root: r.get(2)?,
                    updated_at: Some(time::millis_to_rfc3339(r.get(3)?)),
                    event_count: r.get::<_, i64>(4)? as u64,
                })
            })
            .map_err(src_err)?;
        let mut out = Vec::new();
        for row in rows {
            let s = row.map_err(src_err)?;
            if filter
                .project_root
                .as_ref()
                .is_some_and(|p| s.project_root.as_ref() != Some(p))
            {
                continue;
            }
            out.push(s);
            if filter.limit.is_some_and(|l| out.len() >= l) {
                break;
            }
        }
        Ok(out)
    }

    fn read_session(&self, session_id: &str) -> AdapterResult<Session> {
        let conn = self.open()?;
        let (directory, version, created, updated): (String, String, i64, i64) = conn
            .query_row(
                "SELECT directory, version, time_created, time_updated FROM session_v2 WHERE id = ?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AdapterError::SessionNotFound(session_id.into()),
                e => src_err(e),
            })?;

        let mut stmt = conn
            .prepare("SELECT id, type, seq, time_created, data FROM session_message WHERE session_id = ?1 ORDER BY seq")
            .map_err(src_err)?;
        let msgs = stmt
            .query_map([session_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(src_err)?;

        let mut b = Builder {
            session_id: session_id.into(),
            events: Vec::new(),
            report: ParseReport::default(),
        };
        for m in msgs {
            let (id, typ, seq, ts, data) = m.map_err(src_err)?;
            b.message(&id, &typ, seq as u64, ts, &data);
        }

        Ok(Session {
            id: session_id.into(),
            agent: AgentInfo {
                id: AgentId::OpenCode,
                version: Some(version),
                adapter_version: ADAPTER_VERSION.into(),
            },
            source: SessionSource {
                path: self.db_path.display().to_string(),
                format: FORMAT.into(),
                format_version: Some(FORMAT_VERSION.into()),
            },
            project: Some(ProjectRef {
                repository: Path::new(&directory).join(".git").exists(),
                root: directory,
            }),
            started_at: Some(time::millis_to_rfc3339(created)),
            updated_at: Some(time::millis_to_rfc3339(updated)),
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

struct Builder {
    session_id: String,
    events: Vec<Event>,
    report: ParseReport,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        native_id: &str,
        ts: i64,
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
            timestamp: Some(time::millis_to_rfc3339(ts)),
            kind,
            actor,
            content,
            tool,
            status,
            source_ref: EventSourceRef {
                session_id: self.session_id.clone(),
                native_event_id: Some(native_id.into()),
            },
            parse_status: parse,
        });
        self.report.parsed_events += 1;
    }

    fn unsupported(&mut self, position: u64, kind: &str, reason: &str) {
        self.report.total_events += 1;
        self.report.unsupported.push(UnsupportedEvent {
            position,
            native_kind: Some(kind.into()),
            reason: reason.into(),
        });
    }

    fn text(t: Option<&str>) -> EventContent {
        EventContent {
            text: t.map(String::from),
            structured: None,
        }
    }

    fn message(&mut self, id: &str, typ: &str, seq: u64, ts: i64, data: &str) {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                self.report.total_events += 1;
                self.report
                    .errors
                    .push(format!("message {id} (seq {seq}): invalid JSON: {e}"));
                return;
            }
        };
        match typ {
            "user" => {
                self.report.total_events += 1;
                let attachments = v.get("files").and_then(Value::as_array).map_or(0, Vec::len);
                let structured =
                    (attachments > 0).then(|| json!({ "attachment_count": attachments }));
                let content = EventContent {
                    text: v["text"].as_str().map(String::from),
                    structured,
                };
                self.push(
                    id,
                    ts,
                    EventKind::UserMessage,
                    Actor::User,
                    content,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Parsed,
                );
            }
            "assistant" => self.assistant(id, seq, ts, &v),
            "system" | "synthetic" => {
                self.report.total_events += 1;
                self.push(
                    id,
                    ts,
                    EventKind::Unknown,
                    Actor::System,
                    Self::text(v["text"].as_str()),
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Partial,
                );
            }
            "idle" => {
                self.report.total_events += 1;
                if let Some(err) = v.get("error") {
                    let msg = err["message"].as_str().map(String::from);
                    let structured = Some(json!({ "type": err["type"], "outcome": v["outcome"] }));
                    self.push(
                        id,
                        ts,
                        EventKind::Error,
                        Actor::System,
                        EventContent {
                            text: msg,
                            structured,
                        },
                        None,
                        ToolStatus::Unknown,
                        ParseStatus::Parsed,
                    );
                } else {
                    self.report.parsed_events += 1;
                }
            }
            "agent-switched" | "model-switched" => {
                self.report.total_events += 1;
                let content = EventContent {
                    text: None,
                    structured: Some(v.clone()),
                };
                self.push(
                    id,
                    ts,
                    EventKind::Unknown,
                    Actor::System,
                    content,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Partial,
                );
            }
            other => self.unsupported(seq, other, "unrecognised message type"),
        }
    }

    fn assistant(&mut self, id: &str, seq: u64, ts: i64, v: &Value) {
        let Some(items) = v.get("content").and_then(Value::as_array) else {
            self.unsupported(seq, "assistant", "missing content array");
            return;
        };
        if let Some(err) = v.get("error") {
            self.report.total_events += 1;
            let content = EventContent {
                text: err["message"].as_str().map(String::from),
                structured: Some(err.clone()),
            };
            self.push(
                id,
                ts,
                EventKind::Error,
                Actor::Assistant,
                content,
                None,
                ToolStatus::Unknown,
                ParseStatus::Parsed,
            );
        }
        for item in items {
            match item["type"].as_str() {
                // Hidden model reasoning is deliberately not transferred (spec non-goal).
                Some("reasoning") => {}
                Some("text") => {
                    self.report.total_events += 1;
                    self.push(
                        id,
                        ts,
                        EventKind::AssistantMessage,
                        Actor::Assistant,
                        Self::text(item["text"].as_str()),
                        None,
                        ToolStatus::Unknown,
                        ParseStatus::Parsed,
                    );
                }
                Some("tool") => self.tool(id, ts, item),
                other => self.unsupported(
                    seq,
                    other.unwrap_or("none"),
                    "unrecognised assistant content item",
                ),
            }
        }
    }

    fn tool(&mut self, id: &str, ts: i64, item: &Value) {
        let name = item["name"].as_str().unwrap_or("unknown").to_string();
        let state = &item["state"];
        let input = state.get("input").cloned();
        self.report.total_events += 1;
        self.push(
            id,
            ts,
            EventKind::ToolCall,
            Actor::Assistant,
            EventContent::default(),
            Some(ToolInfo {
                name: name.clone(),
                input: input.clone(),
                output: None,
            }),
            ToolStatus::Unknown,
            ParseStatus::Parsed,
        );

        let output: String = state["content"]
            .as_array()
            .map(|c| {
                c.iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        // A recorded call is not a success: derive status from explicit result data only.
        let status = match (state["status"].as_str(), state["metadata"]["exit"].as_i64()) {
            (Some("completed"), Some(0)) => ToolStatus::Succeeded,
            (Some("completed"), Some(_)) => ToolStatus::Failed,
            (Some("completed"), None) => ToolStatus::Succeeded,
            (Some("error"), _) => ToolStatus::Failed,
            _ => ToolStatus::Unknown,
        };
        let output = if status == ToolStatus::Failed && output.is_empty() {
            state["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        } else {
            output
        };
        self.report.total_events += 1;
        self.push(
            id,
            ts,
            EventKind::ToolResult,
            Actor::Tool,
            EventContent::default(),
            Some(ToolInfo {
                name: name.clone(),
                input: input.clone(),
                output: (!output.is_empty()).then_some(output),
            }),
            status,
            ParseStatus::Parsed,
        );

        if matches!(name.as_str(), "edit" | "write") && status == ToolStatus::Succeeded {
            if let Some(path) = input.as_ref().and_then(|i| i["path"].as_str()) {
                self.report.total_events += 1;
                let content = EventContent {
                    text: None,
                    structured: Some(json!({ "path": path, "tool": name })),
                };
                self.push(
                    id,
                    ts,
                    EventKind::FileChange,
                    Actor::Tool,
                    content,
                    None,
                    ToolStatus::Succeeded,
                    ParseStatus::Parsed,
                );
            }
        }
    }
}
