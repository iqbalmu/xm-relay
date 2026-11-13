//! Interactive keyboard-first TUI for xm-relay (spec section 14).
//!
//! Design rules enforced:
//! - Status is always indicated via explicit text labels (`[PASS]`, `[WARN]`, `[BLOCKED]`),
//!   never by colour alone.
//! - Qualitative confidence (`High`, `Medium`, `Low`, `Unknown`), never fake percentages.
//! - Read-only browsing with explicit action keys to create checkpoints or handoffs.

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::{Frame, Terminal};
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use xm_relay_checkpoint::{
    create_checkpoint, default_store_dir as default_ckpt_dir, list_checkpoints, CheckpointManifest,
};
use xm_relay_core::{
    AgentAdapter, AgentId, AgentInstallation, EventKind, Session, SessionFilter, SessionSummary,
};
use xm_relay_handoff::{
    create_handoff, default_store_dir as default_ho_dir, list_handoffs, validate_handoff,
    HandoffDetail, HandoffManifest, PreflightStatus,
};
use xm_relay_repository::{discover_instructions, inspect, InstructionSet, RepositoryState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Sessions,
    SessionDetail,
    Project,
    Handoffs,
    Agents,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Sessions,
        Tab::SessionDetail,
        Tab::Project,
        Tab::Handoffs,
        Tab::Agents,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Sessions => "1:Sessions",
            Tab::SessionDetail => "2:Session Detail",
            Tab::Project => "3:Project & Checkpoints",
            Tab::Handoffs => "4:Handoffs & Preflight",
            Tab::Agents => "5:Agents",
        }
    }

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

pub struct App {
    adapters: Vec<Box<dyn AgentAdapter>>,
    pub tab: Tab,
    pub agent_filter: Option<AgentId>,
    pub target_agent: AgentId,
    pub search_mode: bool,
    pub search_query: String,
    pub status_msg: String,
    pub should_quit: bool,

    pub installations: Vec<AgentInstallation>,
    pub all_sessions: Vec<SessionSummary>,
    pub filtered_sessions: Vec<SessionSummary>,
    pub session_index: usize,
    pub loaded_session: Option<Session>,
    pub detail_scroll: u16,

    pub project_path: PathBuf,
    pub repo_state: RepositoryState,
    pub instructions: InstructionSet,
    pub checkpoints: Vec<CheckpointManifest>,

    pub handoffs: Vec<HandoffManifest>,
    pub handoff_index: usize,
    pub loaded_handoff: Option<HandoffDetail>,
    pub handoff_scroll: u16,
}

impl App {
    pub fn new(adapters: Vec<Box<dyn AgentAdapter>>, project_path: PathBuf) -> Self {
        let mut app = Self {
            adapters,
            tab: Tab::Sessions,
            agent_filter: None,
            target_agent: AgentId::Omp,
            search_mode: false,
            search_query: String::new(),
            status_msg: "Ready. Keys: Tab/1-5 switch views | j/k move | Enter inspect | / search | a filter | t target | h handoff | c checkpoint | q quit".into(),
            should_quit: false,
            installations: Vec::new(),
            all_sessions: Vec::new(),
            filtered_sessions: Vec::new(),
            session_index: 0,
            loaded_session: None,
            detail_scroll: 0,
            project_path,
            repo_state: RepositoryState::default(),
            instructions: InstructionSet::default(),
            checkpoints: Vec::new(),
            handoffs: Vec::new(),
            handoff_index: 0,
            loaded_handoff: None,
            handoff_scroll: 0,
        };
        app.refresh();
        app
    }

    pub fn refresh(&mut self) {
        self.installations.clear();
        self.all_sessions.clear();
        let filter = SessionFilter {
            limit: Some(100),
            ..Default::default()
        };
        for a in &self.adapters {
            if let Ok(inst) = a.detect() {
                self.installations.push(inst);
            }
            if let Ok(list) = a.list_sessions(&filter) {
                self.all_sessions.extend(list);
            }
        }
        self.all_sessions
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        self.apply_filter();

        if let Ok(st) = inspect(&self.project_path) {
            self.repo_state = st;
        }
        self.instructions = discover_instructions(&self.project_path);
        if let Some(dir) = default_ckpt_dir() {
            self.checkpoints = list_checkpoints(&dir, None).unwrap_or_default();
        }
        if let Some(dir) = default_ho_dir() {
            self.handoffs = list_handoffs(&dir).unwrap_or_default();
            if let Some(first) = self.handoffs.get(self.handoff_index) {
                self.loaded_handoff = validate_handoff(&dir, &first.handoff_id, None).ok();
            } else {
                self.loaded_handoff = None;
            }
        }
    }

    pub fn apply_filter(&mut self) {
        let q = self.search_query.to_ascii_lowercase();
        self.filtered_sessions = self
            .all_sessions
            .iter()
            .filter(|s| self.agent_filter.is_none_or(|af| s.agent == af))
            .filter(|s| {
                if q.is_empty() {
                    return true;
                }
                s.id.to_ascii_lowercase().contains(&q)
                    || s.title
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&q)
                    || s.project_root
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&q)
            })
            .cloned()
            .collect();
        if self.session_index >= self.filtered_sessions.len() {
            self.session_index = 0;
        }
    }

    pub fn cycle_agent_filter(&mut self) {
        self.agent_filter = match self.agent_filter {
            None => Some(AgentId::OpenCode),
            Some(AgentId::OpenCode) => Some(AgentId::AntigravityCli),
            Some(AgentId::AntigravityCli) => Some(AgentId::Omp),
            Some(AgentId::Omp) => None,
        };
        self.apply_filter();
    }

    pub fn cycle_target_agent(&mut self) {
        self.target_agent = match self.target_agent {
            AgentId::OpenCode => AgentId::AntigravityCli,
            AgentId::AntigravityCli => AgentId::Omp,
            AgentId::Omp => AgentId::OpenCode,
        };
        self.status_msg = format!("Target agent set to: {}", self.target_agent.as_str());
    }

    pub fn load_selected_session(&mut self) {
        let Some(summary) = self.filtered_sessions.get(self.session_index).cloned() else {
            self.status_msg = "No session selected.".into();
            return;
        };
        for a in &self.adapters {
            if a.id() == summary.agent {
                match a.read_session(&summary.id) {
                    Ok(s) => {
                        self.status_msg = format!(
                            "Loaded session {} ({} events, lossy={})",
                            s.id,
                            s.events.len(),
                            s.parse_report.is_lossy()
                        );
                        self.loaded_session = Some(s);
                        self.detail_scroll = 0;
                        self.tab = Tab::SessionDetail;
                        return;
                    }
                    Err(e) => {
                        self.status_msg = format!("Error loading session: {e}");
                        return;
                    }
                }
            }
        }
    }

    pub fn build_handoff_for_selected(&mut self) {
        if self.loaded_session.is_none() {
            self.load_selected_session();
        }
        let Some(sess) = &self.loaded_session else {
            return;
        };
        let Some(dir) = default_ho_dir() else {
            self.status_msg = "Cannot resolve handoff store directory.".into();
            return;
        };
        match create_handoff(sess, self.target_agent, None, &dir) {
            Ok(detail) => {
                self.status_msg = format!(
                    "Created handoff {} -> {} [status: {:?}, confidence: {:?}]",
                    detail.manifest.handoff_id,
                    detail.manifest.target_agent.as_str(),
                    detail.validation.status,
                    detail.manifest.confidence.overall
                );
                self.handoffs = list_handoffs(&dir).unwrap_or_default();
                self.handoff_index = 0;
                self.loaded_handoff = Some(detail);
                self.handoff_scroll = 0;
                self.tab = Tab::Handoffs;
            }
            Err(e) => {
                self.status_msg = format!("Handoff creation failed: {e}");
            }
        }
    }

    pub fn create_workspace_checkpoint(&mut self) {
        let Some(dir) = default_ckpt_dir() else {
            self.status_msg = "Cannot resolve checkpoint store directory.".into();
            return;
        };
        match create_checkpoint(&self.project_path, &dir) {
            Ok(m) => {
                self.status_msg = format!(
                    "Created checkpoint {} (dirty={}, warnings={})",
                    m.checkpoint_id,
                    m.working_tree_dirty,
                    m.warnings.len()
                );
                self.checkpoints = list_checkpoints(&dir, None).unwrap_or_default();
                self.tab = Tab::Project;
            }
            Err(e) => {
                self.status_msg = format!("Checkpoint failed: {e}");
            }
        }
    }

    pub fn handle_key(&mut self, code: KeyCode) {
        if self.search_mode {
            match code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.search_mode = false;
                    self.status_msg = format!(
                        "Search filter: '{}' ({} sessions)",
                        self.search_query,
                        self.filtered_sessions.len()
                    );
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.apply_filter();
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.apply_filter();
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Tab => self.tab = self.tab.next(),
            KeyCode::BackTab => self.tab = self.tab.prev(),
            KeyCode::Char('1') => self.tab = Tab::Sessions,
            KeyCode::Char('2') => self.tab = Tab::SessionDetail,
            KeyCode::Char('3') => self.tab = Tab::Project,
            KeyCode::Char('4') => self.tab = Tab::Handoffs,
            KeyCode::Char('5') => self.tab = Tab::Agents,
            KeyCode::Char('/') => {
                self.search_mode = true;
                self.tab = Tab::Sessions;
                self.status_msg = "Type search query and press Enter/Esc...".into();
            }
            KeyCode::Char('a') => {
                self.cycle_agent_filter();
                self.status_msg = format!(
                    "Agent filter: {}",
                    self.agent_filter.map(|a| a.as_str()).unwrap_or("ALL")
                );
            }
            KeyCode::Char('t') => self.cycle_target_agent(),
            KeyCode::Char('r') => {
                self.refresh();
                self.status_msg =
                    "Refreshed sessions, repository, checkpoints, and handoffs.".into();
            }
            KeyCode::Char('c') => self.create_workspace_checkpoint(),
            KeyCode::Char('h') => self.build_handoff_for_selected(),
            KeyCode::Enter => {
                if self.tab == Tab::Sessions {
                    self.load_selected_session();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => match self.tab {
                Tab::Sessions if !self.filtered_sessions.is_empty() => {
                    self.session_index = (self.session_index + 1) % self.filtered_sessions.len();
                }
                Tab::SessionDetail => self.detail_scroll = self.detail_scroll.saturating_add(2),
                Tab::Handoffs if !self.handoffs.is_empty() => {
                    self.handoff_index = (self.handoff_index + 1) % self.handoffs.len();
                    if let Some(dir) = default_ho_dir() {
                        if let Some(h) = self.handoffs.get(self.handoff_index) {
                            self.loaded_handoff = validate_handoff(&dir, &h.handoff_id, None).ok();
                            self.handoff_scroll = 0;
                        }
                    }
                }
                _ => {}
            },
            KeyCode::Up | KeyCode::Char('k') => match self.tab {
                Tab::Sessions if !self.filtered_sessions.is_empty() => {
                    self.session_index = (self.session_index + self.filtered_sessions.len() - 1)
                        % self.filtered_sessions.len();
                }
                Tab::SessionDetail => self.detail_scroll = self.detail_scroll.saturating_sub(2),
                Tab::Handoffs if !self.handoffs.is_empty() => {
                    self.handoff_index =
                        (self.handoff_index + self.handoffs.len() - 1) % self.handoffs.len();
                    if let Some(dir) = default_ho_dir() {
                        if let Some(h) = self.handoffs.get(self.handoff_index) {
                            self.loaded_handoff = validate_handoff(&dir, &h.handoff_id, None).ok();
                            self.handoff_scroll = 0;
                        }
                    }
                }
                _ => {}
            },
            KeyCode::PageDown => {
                if self.tab == Tab::SessionDetail {
                    self.detail_scroll = self.detail_scroll.saturating_add(10);
                } else if self.tab == Tab::Handoffs {
                    self.handoff_scroll = self.handoff_scroll.saturating_add(10);
                }
            }
            KeyCode::PageUp => {
                if self.tab == Tab::SessionDetail {
                    self.detail_scroll = self.detail_scroll.saturating_sub(10);
                } else if self.tab == Tab::Handoffs {
                    self.handoff_scroll = self.handoff_scroll.saturating_sub(10);
                }
            }
            _ => {}
        }
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    let titles: Vec<Line> = Tab::ALL.iter().map(|t| Line::from(t.title())).collect();
    let idx = Tab::ALL.iter().position(|&t| t == app.tab).unwrap_or(0);
    let header_title = format!(
        " xm-relay | Filter: {} | Target Agent: {} | Search: '{}' ",
        app.agent_filter.map(|a| a.as_str()).unwrap_or("ALL"),
        app.target_agent.as_str(),
        app.search_query
    );
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(header_title))
        .select(idx)
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[0]);

    match app.tab {
        Tab::Sessions => draw_sessions(f, app, chunks[1]),
        Tab::SessionDetail => draw_session_detail(f, app, chunks[1]),
        Tab::Project => draw_project(f, app, chunks[1]),
        Tab::Handoffs => draw_handoffs(f, app, chunks[1]),
        Tab::Agents => draw_agents(f, app, chunks[1]),
    }

    let footer = Paragraph::new(app.status_msg.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Status & Controls "),
    );
    f.render_widget(footer, chunks[2]);
}

fn draw_sessions(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .filtered_sessions
        .iter()
        .map(|s| {
            let line = format!(
                "[{:<15}] {:<36}  {}  {:>4} ev  {}  {}",
                s.agent.as_str(),
                s.id,
                s.updated_at.as_deref().unwrap_or("-"),
                s.event_count,
                s.project_root.as_deref().unwrap_or("-"),
                s.title.as_deref().unwrap_or("(untitled)")
            );
            ListItem::new(line)
        })
        .collect();

    let title = format!(
        " Sessions ({}/{}) — Press Enter to inspect, 'h' to build handoff -> {} ",
        app.filtered_sessions.len(),
        app.all_sessions.len(),
        app.target_agent.as_str()
    );
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    let mut state = ListState::default();
    if !app.filtered_sessions.is_empty() {
        state.select(Some(app.session_index));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_session_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(s) = &app.loaded_session else {
        let p =
            Paragraph::new("No session loaded. Select a session in 1:Sessions and press Enter.")
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Session Detail "),
                );
        f.render_widget(p, area);
        return;
    };

    let r = &s.parse_report;
    let parse_tag = if r.is_lossy() {
        "[WARN: LOSSY PARSE]"
    } else {
        "[OK: COMPLETE PARSE]"
    };
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                "Session ID: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(&s.id),
            Span::raw("   Agent: "),
            Span::raw(s.agent.id.as_str()),
            Span::raw("   Version: "),
            Span::raw(s.agent.version.as_deref().unwrap_or("unknown")),
        ]),
        Line::from(format!(
            "Project:    {}",
            s.project
                .as_ref()
                .map(|p| p.root.as_str())
                .unwrap_or("unknown")
        )),
        Line::from(format!(
            "Parse:      {parse_tag} total={} parsed={} unsupported={} errors={}",
            r.total_events,
            r.parsed_events,
            r.unsupported_events(),
            r.errors.len()
        )),
        Line::from("----------------------------------------------------------------------"),
    ];

    for u in &r.unsupported {
        lines.push(Line::from(format!(
            "  [UNSUPPORTED @ {}] kind={:?} reason={}",
            u.position, u.native_kind, u.reason
        )));
    }
    for e in &r.errors {
        lines.push(Line::from(format!("  [PARSE ERROR] {e}")));
    }

    for ev in &s.events {
        let summary = match ev.kind {
            EventKind::UserMessage
            | EventKind::AssistantMessage
            | EventKind::Compaction
            | EventKind::Checkpoint
            | EventKind::Error => ev
                .content
                .text
                .as_deref()
                .and_then(|t| t.lines().next())
                .unwrap_or("")
                .chars()
                .take(90)
                .collect::<String>(),
            EventKind::ToolCall | EventKind::ToolResult => {
                let name = ev.tool.as_ref().map(|t| t.name.as_str()).unwrap_or("tool");
                format!("{name} [status: {:?}]", ev.status)
            }
            EventKind::FileChange => ev
                .content
                .structured
                .as_ref()
                .and_then(|v| v["path"].as_str())
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        };
        lines.push(Line::from(format!(
            "#{:<4} [{:<16?}] ({:<9?}) {}",
            ev.sequence, ev.kind, ev.actor, summary
        )));
    }

    let p =
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(
                " Session Detail & Timeline (j/k/PgUp/PgDn to scroll, 'h' to build handoff) ",
            ))
            .scroll((app.detail_scroll, 0))
            .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn draw_project(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let st = &app.repo_state;
    let state_tag = if !st.is_repository {
        "[NOT A GIT REPO]"
    } else if !st.conflicted.is_empty() {
        "[CONFLICTED]"
    } else if st.working_tree_dirty {
        "[DIRTY]"
    } else {
        "[CLEAN]"
    };

    let mut left_lines = vec![
        Line::from(format!("Path:       {}", app.project_path.display())),
        Line::from(format!("Repo Root:  {}", st.root.as_deref().unwrap_or("-"))),
        Line::from(format!("Status:     {state_tag}")),
        Line::from(format!(
            "Branch:     {}   HEAD: {}",
            st.branch.as_deref().unwrap_or("(detached/none)"),
            st.head_commit.as_deref().unwrap_or("(none)")
        )),
        Line::from(format!(
            "Changes:    staged={} unstaged={} untracked={} conflicted={}",
            st.staged.len(),
            st.unstaged.len(),
            st.untracked.len(),
            st.conflicted.len()
        )),
        Line::from(format!(
            "Diff Stat:  {} files (+{} -{})",
            st.diff_summary.files.len(),
            st.diff_summary.additions,
            st.diff_summary.deletions
        )),
        Line::from(""),
        Line::from("Project Instructions:"),
    ];
    if app.instructions.files.is_empty() {
        left_lines.push(Line::from("  (none discovered)"));
    } else {
        for ins in &app.instructions.files {
            left_lines.push(Line::from(format!(
                "  - {} ({} bytes, applies={})",
                ins.path, ins.size, ins.applies_to_start
            )));
        }
    }

    let left = Paragraph::new(left_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Live Workspace State (press 'c' to create checkpoint) "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(left, cols[0]);

    let ckpt_items: Vec<ListItem> = app
        .checkpoints
        .iter()
        .map(|c| {
            let tag = if c.working_tree_dirty {
                "[DIRTY]"
            } else {
                "[CLEAN]"
            };
            ListItem::new(format!(
                "{}  {}  {}  {}",
                c.checkpoint_id,
                c.created_at,
                c.branch.as_deref().unwrap_or("(detached)"),
                tag
            ))
        })
        .collect();
    let right = List::new(ckpt_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Checkpoints ({}) ", app.checkpoints.len())),
    );
    f.render_widget(right, cols[1]);
}

fn draw_handoffs(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = app
        .handoffs
        .iter()
        .map(|h| {
            ListItem::new(format!(
                "{}  {} -> {}  [conf: {:?}]",
                h.handoff_id,
                h.source_agent.as_str(),
                h.target_agent.as_str(),
                h.confidence.overall
            ))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Handoff Packages ({}) ", app.handoffs.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    let mut state = ListState::default();
    if !app.handoffs.is_empty() {
        state.select(Some(app.handoff_index));
    }
    f.render_stateful_widget(list, cols[0], &mut state);

    let Some(d) = &app.loaded_handoff else {
        let p = Paragraph::new("No handoff selected. Press 'h' on a session to build one.").block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Validation & Context "),
        );
        f.render_widget(p, cols[1]);
        return;
    };

    let status_label = match d.validation.status {
        PreflightStatus::Passed => "[PASS]",
        PreflightStatus::PassedWithWarnings => "[PASS WITH WARNINGS]",
        PreflightStatus::Blocked => "[BLOCKED]",
        PreflightStatus::Failed => "[FAILED]",
    };

    let mut lines: Vec<Line> = vec![
        Line::from(format!(
            "Preflight Status: {}   Checksums: {}",
            status_label,
            if d.validation.checksums_valid {
                "VALID"
            } else {
                "INVALID"
            }
        )),
        Line::from(format!(
            "Confidence:       {:?} — {}",
            d.manifest.confidence.overall, d.manifest.confidence.reason
        )),
        Line::from(""),
        Line::from("Preflight Checklist:"),
    ];
    for c in &d.validation.checklist {
        lines.push(Line::from(format!(
            "  [{}] {} ({})",
            if c.checked { "x" } else { " " },
            c.label,
            c.detail
        )));
    }
    if !d.validation.conflicts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("Conflicts:"));
        for c in &d.validation.conflicts {
            lines.push(Line::from(format!(
                "  [CONFLICT {} {:?}] {}",
                c.id, c.kind, c.description
            )));
        }
    }
    lines.push(Line::from(
        "----------------------------------------------------------------------",
    ));
    for l in d.context_md.lines() {
        lines.push(Line::from(l.to_string()));
    }

    let right = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Preflight Validation & Context (PgUp/PgDn to scroll) "),
        )
        .scroll((app.handoff_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(right, cols[1]);
}

fn draw_agents(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .installations
        .iter()
        .map(|i| {
            let tag = if i.available {
                "[FOUND]"
            } else {
                "[NOT FOUND]"
            };
            ListItem::new(format!(
                "{:<16} {:<12} version={:<10} path={}",
                i.agent.as_str(),
                tag,
                i.version.as_deref().unwrap_or("unknown"),
                i.data_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "-".into())
            ))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Detected Agent Installations "),
    );
    f.render_widget(list, area);
}

/// Launch the interactive terminal UI.
pub fn run_tui(adapters: Vec<Box<dyn AgentAdapter>>, project_path: PathBuf) -> Result<(), String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let mut app = App::new(adapters, project_path);
    let res = event_loop(&mut terminal, &mut app);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    res
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<(), String> {
    while !app.should_quit {
        terminal.draw(|f| draw(f, app)).map_err(|e| e.to_string())?;
        if event::poll(Duration::from_millis(200)).map_err(|e| e.to_string())? {
            if let CEvent::Key(key) = event::read().map_err(|e| e.to_string())? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_all_tabs_and_handles_navigation_in_memory() {
        let backend = TestBackend::new(120, 35);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(Vec::new(), PathBuf::from("."));

        for _ in 0..Tab::ALL.len() {
            terminal.draw(|f| draw(f, &app)).unwrap();
            app.handle_key(KeyCode::Tab);
        }

        app.handle_key(KeyCode::Char('/'));
        assert!(app.search_mode);
        app.handle_key(KeyCode::Char('a'));
        app.handle_key(KeyCode::Char('u'));
        app.handle_key(KeyCode::Enter);
        assert!(!app.search_mode);
        assert_eq!(app.search_query, "au");

        app.handle_key(KeyCode::Char('t'));
        assert_eq!(app.target_agent, AgentId::OpenCode);

        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit);
    }
}
