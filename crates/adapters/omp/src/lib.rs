//! Read-only Oh My Pi (`omp`) adapter. Source: JSONL session files at
//! `<agent dir>/sessions/<cwd-slug>/<timestamp>_<uuid>.jsonl`. Never writes.

use serde_json::{json, Value};
use std::collections::HashMap;
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
const FORMAT: &str = "omp-jsonl";
/// Entry types that are bookkeeping/telemetry with no handoff value.
const IGNORED: &[&str] = &[
    "custom",
    "thinking_level_change",
    "title_change",
    "title",
    "model_usage",
    "service_tier_change",
    "credential_pin",
    "ttsr_injection",
    "mode_change",
];

pub struct OmpAdapter {
    sessions_dir: PathBuf,
}

impl OmpAdapter {
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Self {
        Self {
            sessions_dir: sessions_dir.into(),
        }
    }

    /// `~/.omp/agent/sessions`.
    pub fn default_sessions_dir() -> Option<PathBuf> {
        Some(Path::new(&std::env::var_os("HOME")?).join(".omp/agent/sessions"))
    }

    fn files(&self) -> AdapterResult<Vec<PathBuf>> {
        let rd = std::fs::read_dir(&self.sessions_dir).map_err(|e| {
            AdapterError::Unavailable(format!("{}: {e}", self.sessions_dir.display()))
        })?;
        let mut out = Vec::new();
        for dir in rd.flatten().filter(|d| d.path().is_dir()) {
            if let Ok(inner) = std::fs::read_dir(dir.path()) {
                out.extend(
                    inner
                        .flatten()
                        .map(|f| f.path())
                        .filter(|p| p.extension().is_some_and(|e| e == "jsonl")),
                );
            }
        }
        Ok(out)
    }

    fn find(&self, id: &str) -> AdapterResult<PathBuf> {
        // File names end with `_<uuid>.jsonl`; the header id is authoritative but must match.
        let suffix = format!("_{id}.jsonl");
        self.files()?
            .into_iter()
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(&suffix))
            })
            .ok_or_else(|| AdapterError::SessionNotFound(id.into()))
    }
}

fn io_err(p: &Path, e: std::io::Error) -> AdapterError {
    AdapterError::Source(format!("{}: {e}", p.display()))
}

impl AgentAdapter for OmpAdapter {
    fn id(&self) -> AgentId {
        AgentId::Omp
    }

    fn detect(&self) -> AdapterResult<AgentInstallation> {
        let available = self.sessions_dir.is_dir();
        Ok(AgentInstallation {
            agent: AgentId::Omp,
            available,
            data_path: available.then(|| self.sessions_dir.clone()),
            // Per-entry `runtime.version` exists but the CLI is not executed to detect it.
            version: None,
        })
    }

    fn list_sessions(&self, filter: &SessionFilter) -> AdapterResult<Vec<SessionSummary>> {
        let mut out = Vec::new();
        for path in self.files()? {
            let f = File::open(&path).map_err(|e| io_err(&path, e))?;
            let (mut id, mut cwd, mut title, mut updated, mut count) =
                (None, None, None, None, 0u64);
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match v["type"].as_str() {
                    Some("session") => {
                        id = v["id"].as_str().map(String::from);
                        cwd = v["cwd"].as_str().map(String::from);
                        title = v["title"].as_str().map(String::from).or(title);
                    }
                    Some("title") | Some("title_change") => {
                        title = v["title"].as_str().map(String::from).or(title)
                    }
                    Some("message") | Some("compaction") => count += 1,
                    _ => {}
                }
                if let Some(t) = v["timestamp"].as_str() {
                    updated = Some(t.to_string());
                }
            }
            let Some(id) = id else { continue };
            if filter
                .project_root
                .as_ref()
                .is_some_and(|p| cwd.as_ref() != Some(p))
            {
                continue;
            }
            out.push(SessionSummary {
                id,
                agent: AgentId::Omp,
                title,
                project_root: cwd,
                updated_at: updated,
                event_count: count,
            });
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        if let Some(l) = filter.limit {
            out.truncate(l);
        }
        Ok(out)
    }

    fn read_session(&self, session_id: &str) -> AdapterResult<Session> {
        let path = self.find(session_id)?;
        let f = File::open(&path).map_err(|e| io_err(&path, e))?;
        let mut b = Builder {
            session_id: session_id.into(),
            events: Vec::new(),
            report: ParseReport::default(),
            calls: HashMap::new(),
        };
        let (mut cwd, mut started, mut updated, mut fmt_version) = (None, None, None, None);

        for (i, line) in BufReader::new(f).lines().enumerate() {
            let pos = i as u64 + 1;
            let line = line.map_err(|e| io_err(&path, e))?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    b.report.total_events += 1;
                    b.report
                        .errors
                        .push(format!("line {pos}: invalid JSON: {e}"));
                    continue;
                }
            };
            if let Some(t) = v["timestamp"].as_str() {
                updated = Some(t.to_string());
            }
            let typ = v["type"].as_str().unwrap_or("");
            if typ == "session" {
                cwd = v["cwd"].as_str().map(String::from);
                started = v["timestamp"].as_str().map(String::from);
                fmt_version = v["version"].as_i64().map(|n| n.to_string());
                continue;
            }
            b.entry(pos, typ, &v);
        }

        let root =
            cwd.ok_or_else(|| AdapterError::UnsupportedFormat("missing session header".into()))?;
        Ok(Session {
            id: session_id.into(),
            agent: AgentInfo {
                id: AgentId::Omp,
                version: None,
                adapter_version: ADAPTER_VERSION.into(),
            },
            source: SessionSource {
                path: path.display().to_string(),
                format: FORMAT.into(),
                format_version: fmt_version,
            },
            project: Some(ProjectRef {
                repository: Path::new(&root).join(".git").exists(),
                root,
            }),
            started_at: started,
            updated_at: updated,
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
    /// toolCallId -> (name, arguments), to attach inputs to results.
    calls: HashMap<String, (String, Value)>,
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

    fn unsupported(&mut self, pos: u64, kind: &str, reason: &str) {
        self.report.total_events += 1;
        self.report.unsupported.push(UnsupportedEvent {
            position: pos,
            native_kind: Some(kind.into()),
            reason: reason.into(),
        });
    }

    fn entry(&mut self, pos: u64, typ: &str, v: &Value) {
        let id = v["id"].as_str().unwrap_or("").to_string();
        let ts = v["timestamp"].as_str();
        match typ {
            "message" => self.message(pos, &id, ts, &v["message"]),
            "compaction" => {
                self.report.total_events += 1;
                let structured = json!({
                    "tokens_before": v["tokensBefore"], "tokens_after": v["tokensAfter"],
                    "first_kept_entry_id": v["firstKeptEntryId"], "method": v["method"],
                });
                let content = EventContent {
                    text: v["summary"].as_str().map(String::from),
                    structured: Some(structured),
                };
                self.push(
                    &id,
                    ts,
                    EventKind::Compaction,
                    Actor::System,
                    content,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Parsed,
                );
            }
            "model_change" => {
                self.report.total_events += 1;
                let content = EventContent {
                    text: None,
                    structured: Some(json!({ "model": v["model"], "role": v["role"] })),
                };
                self.push(
                    &id,
                    ts,
                    EventKind::Unknown,
                    Actor::System,
                    content,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Partial,
                );
            }
            "custom_message" => {
                self.report.total_events += 1;
                let content = EventContent {
                    text: text_of(&v["content"]),
                    structured: Some(json!({ "custom_type": v["customType"] })),
                };
                self.push(
                    &id,
                    ts,
                    EventKind::Unknown,
                    Actor::System,
                    content,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Partial,
                );
            }
            t if IGNORED.contains(&t) => {}
            other => self.unsupported(pos, other, "unrecognised entry type"),
        }
    }

    fn message(&mut self, pos: u64, id: &str, ts: Option<&str>, m: &Value) {
        let role = m["role"].as_str().unwrap_or("");
        match role {
            "user" => {
                self.report.total_events += 1;
                let images = m["content"]
                    .as_array()
                    .map_or(0, |a| a.iter().filter(|b| b["type"] == "image").count());
                let structured = (images > 0).then(|| json!({ "attachment_count": images }));
                let content = EventContent {
                    text: text_of(&m["content"]),
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
            "assistant" => self.assistant(pos, id, ts, m),
            "toolResult" => self.tool_result(id, ts, m),
            "bashExecution" => {
                self.report.total_events += 1;
                let status = if m["cancelled"] == true {
                    ToolStatus::Cancelled
                } else {
                    match m["exitCode"].as_i64() {
                        Some(0) => ToolStatus::Succeeded,
                        Some(_) => ToolStatus::Failed,
                        None => ToolStatus::Unknown,
                    }
                };
                let tool = ToolInfo {
                    name: "bash".into(),
                    input: Some(json!({ "command": m["command"] })),
                    output: m["output"].as_str().map(String::from),
                };
                self.push(
                    id,
                    ts,
                    EventKind::ToolResult,
                    Actor::Tool,
                    EventContent::default(),
                    Some(tool),
                    status,
                    ParseStatus::Parsed,
                );
            }
            "developer" | "custom" => {
                self.report.total_events += 1;
                self.push(
                    id,
                    ts,
                    EventKind::Unknown,
                    Actor::System,
                    EventContent {
                        text: text_of(&m["content"]),
                        structured: None,
                    },
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Partial,
                );
            }
            "fileMention" => {
                self.report.total_events += 1;
                let paths: Vec<&Value> = m["files"]
                    .as_array()
                    .map(|a| a.iter().map(|f| &f["path"]).collect())
                    .unwrap_or_default();
                let content = EventContent {
                    text: None,
                    structured: Some(json!({ "paths": paths })),
                };
                self.push(
                    id,
                    ts,
                    EventKind::Attachment,
                    Actor::User,
                    content,
                    None,
                    ToolStatus::Unknown,
                    ParseStatus::Parsed,
                );
            }
            other => self.unsupported(
                pos,
                &format!("message:{other}"),
                "unrecognised message role",
            ),
        }
    }

    fn assistant(&mut self, pos: u64, id: &str, ts: Option<&str>, m: &Value) {
        let Some(blocks) = m["content"].as_array() else {
            self.unsupported(pos, "assistant", "missing content array");
            return;
        };
        for block in blocks {
            match block["type"].as_str() {
                // Hidden model reasoning is deliberately not transferred (spec non-goal).
                Some("thinking") => {}
                Some("text") => {
                    self.report.total_events += 1;
                    self.push(
                        id,
                        ts,
                        EventKind::AssistantMessage,
                        Actor::Assistant,
                        EventContent {
                            text: block["text"].as_str().map(String::from),
                            structured: None,
                        },
                        None,
                        ToolStatus::Unknown,
                        ParseStatus::Parsed,
                    );
                }
                Some("toolCall") => {
                    self.report.total_events += 1;
                    let name = block["name"].as_str().unwrap_or("unknown").to_string();
                    if let Some(cid) = block["id"].as_str() {
                        self.calls
                            .insert(cid.into(), (name.clone(), block["arguments"].clone()));
                    }
                    let native = block["id"].as_str().unwrap_or(id);
                    let tool = ToolInfo {
                        name,
                        input: Some(block["arguments"].clone()),
                        output: None,
                    };
                    self.push(
                        native,
                        ts,
                        EventKind::ToolCall,
                        Actor::Assistant,
                        EventContent::default(),
                        Some(tool),
                        ToolStatus::Unknown,
                        ParseStatus::Parsed,
                    );
                }
                other => self.unsupported(
                    pos,
                    other.unwrap_or("none"),
                    "unrecognised assistant content block",
                ),
            }
        }
        match m["stopReason"].as_str() {
            Some("error") | Some("aborted") => {
                self.report.total_events += 1;
                let content = EventContent {
                    text: m["errorMessage"].as_str().map(String::from),
                    structured: Some(
                        json!({ "stop_reason": m["stopReason"], "error_status": m["errorStatus"] }),
                    ),
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
            _ => {}
        }
    }

    fn tool_result(&mut self, id: &str, ts: Option<&str>, m: &Value) {
        let cid = m["toolCallId"].as_str().unwrap_or("");
        let name = m["toolName"].as_str().unwrap_or("unknown").to_string();
        let input = self.calls.get(cid).map(|(_, a)| a.clone());
        // Only the explicit `isError` flag decides; a missing flag stays unknown.
        let status = match m["isError"].as_bool() {
            Some(false) => ToolStatus::Succeeded,
            Some(true) => ToolStatus::Failed,
            None => ToolStatus::Unknown,
        };
        let output = text_of(&m["content"]);
        self.report.total_events += 1;
        let tool = ToolInfo {
            name: name.clone(),
            input: input.clone(),
            output,
        };
        self.push(
            if cid.is_empty() { id } else { cid },
            ts,
            EventKind::ToolResult,
            Actor::Tool,
            EventContent::default(),
            Some(tool),
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

/// Joins text blocks of a content value (string or block array).
fn text_of(c: &Value) -> Option<String> {
    match c {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => {
            let t: Vec<&str> = a.iter().filter_map(|b| b["text"].as_str()).collect();
            (!t.is_empty()).then(|| t.join("\n"))
        }
        _ => None,
    }
}
