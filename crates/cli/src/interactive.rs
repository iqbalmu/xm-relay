//! Streamlined interactive arrow-key CLI workflow for `xm-relay`.

use crossterm::cursor::{Hide, MoveToColumn, MoveUp, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use xm_relay_core::{Actor, AgentAdapter, AgentId, EventKind, SessionFilter, SessionSummary};
use xm_relay_handoff as ho;

/// Prompt the user to select an item from `items` using Up/Down/j/k and Enter.
/// Returns `Ok(None)` if the user cancels with `Esc` or `q`.
pub fn select_menu(
    prompt: &str,
    items: &[String],
    initial: usize,
) -> Result<Option<usize>, String> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut idx = initial.min(items.len() - 1);
    let max_visible = 14usize;
    let mut stdout = io::stdout();

    enable_raw_mode().map_err(|e| e.to_string())?;
    let _ = execute!(stdout, Hide);

    let mut drawn_lines = 0usize;
    let res = loop {
        if drawn_lines > 0 {
            let _ = execute!(
                stdout,
                MoveUp(drawn_lines as u16),
                MoveToColumn(0),
                Clear(ClearType::FromCursorDown)
            );
        }

        let start = if items.len() <= max_visible {
            0
        } else {
            idx.saturating_sub(max_visible / 2)
                .min(items.len() - max_visible)
        };
        let end = (start + max_visible).min(items.len());

        let _ = execute!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print("? "),
            ResetColor,
            Print(prompt),
            SetForegroundColor(Color::DarkGrey),
            Print(" (↑/↓ move, Enter select, Esc/q cancel)\r\n"),
            ResetColor
        );
        let mut lines = 1usize;

        for (i, item) in items.iter().enumerate().take(end).skip(start) {
            if i == idx {
                let _ = execute!(
                    stdout,
                    SetForegroundColor(Color::Green),
                    Print(format!("  ❯ {item}\r\n")),
                    ResetColor
                );
            } else {
                let _ = execute!(stdout, Print(format!("    {item}\r\n")));
            }
            lines += 1;
        }
        if items.len() > max_visible {
            let _ = execute!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print(format!("    [{}/{}]\r\n", idx + 1, items.len())),
                ResetColor
            );
            lines += 1;
        }
        let _ = stdout.flush();
        drawn_lines = lines;

        match event::read().map_err(|e| e.to_string())? {
            Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    idx = if idx == 0 { items.len() - 1 } else { idx - 1 };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    idx = (idx + 1) % items.len();
                }
                KeyCode::Enter => break Ok(Some(idx)),
                KeyCode::Esc | KeyCode::Char('q') => break Ok(None),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Ok(None)
                }
                _ => {}
            },
            _ => {}
        }
    };

    if drawn_lines > 0 {
        let _ = execute!(
            stdout,
            MoveUp(drawn_lines as u16),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        );
    }
    let _ = execute!(stdout, Show);
    let _ = disable_raw_mode();

    if let Ok(Some(chosen)) = &res {
        let _ = execute!(
            stdout,
            SetForegroundColor(Color::Green),
            Print("✔ "),
            ResetColor,
            Print(format!("{prompt} › {}\n", items[*chosen]))
        );
    }
    res
}

/// Interactive session picker with direct hotkeys:
/// - `g`: Toggle between Active Directory and Global scope
/// - `a`: Cycle agent filter (`ALL` -> `opencode` -> `antigravity-cli` -> `omp`)
/// - `/`: Inline search filter
pub fn pick_session(
    adapters: &[Box<dyn AgentAdapter>],
    active_root: Option<String>,
) -> Result<Option<(AgentId, String)>, String> {
    let mut global = active_root.is_none();
    let mut agent_filter: Option<AgentId> = None;
    let mut search_query = String::new();
    let mut search_mode = false;
    let mut idx = 0usize;
    let mut title_cache: HashMap<(AgentId, String), String> = HashMap::new();

    let max_visible = 12usize;
    let mut stdout = io::stdout();

    enable_raw_mode().map_err(|e| e.to_string())?;
    let _ = execute!(stdout, Hide);

    let mut drawn_lines = 0usize;
    let mut cached_scope: Option<bool> = None;
    let mut all_sessions: Vec<SessionSummary> = Vec::new();

    let res = loop {
        if cached_scope != Some(global) {
            let filter = SessionFilter {
                project_root: if global { None } else { active_root.clone() },
                limit: Some(80),
            };
            all_sessions.clear();
            for a in adapters {
                if let Ok(list) = a.list_sessions(&filter) {
                    all_sessions.extend(list);
                }
            }
            all_sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            for s in &all_sessions {
                let key = (s.agent, s.id.clone());
                title_cache
                    .entry(key)
                    .or_insert_with(|| resolve_session_title(adapters, s));
            }
            cached_scope = Some(global);
            idx = 0;
        }

        let q = search_query.to_ascii_lowercase();
        let filtered: Vec<&SessionSummary> = all_sessions
            .iter()
            .filter(|s| agent_filter.is_none_or(|af| s.agent == af))
            .filter(|s| {
                if q.is_empty() {
                    return true;
                }
                let title = title_cache
                    .get(&(s.agent, s.id.clone()))
                    .map(String::as_str)
                    .unwrap_or("");
                title.to_ascii_lowercase().contains(&q)
                    || s.agent.as_str().to_ascii_lowercase().contains(&q)
                    || s.project_root
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&q)
            })
            .collect();

        if !filtered.is_empty() && idx >= filtered.len() {
            idx = filtered.len() - 1;
        }

        if drawn_lines > 0 {
            let _ = execute!(
                stdout,
                MoveUp(drawn_lines as u16),
                MoveToColumn(0),
                Clear(ClearType::FromCursorDown)
            );
        }

        let scope_badge = if global {
            "Global (All Directories)".to_string()
        } else {
            let short_dir = short_project_name(active_root.as_deref().unwrap_or("."));
            format!("Active Directory ({short_dir})")
        };
        let agent_badge = agent_filter.map(|a| a.as_str()).unwrap_or("All Agents");

        let _ = execute!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print("? "),
            ResetColor,
            Print("Select a session to hand off "),
            SetForegroundColor(Color::Yellow),
            Print(format!("[{scope_badge} • {agent_badge}]")),
            ResetColor,
            Print("\r\n")
        );
        let mut lines = 1usize;

        if search_mode || !search_query.is_empty() {
            let cursor = if search_mode { "▏" } else { "" };
            let _ = execute!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print("  Search: "),
                SetForegroundColor(Color::White),
                Print(format!("{search_query}{cursor}")),
                SetForegroundColor(Color::DarkGrey),
                Print(if search_mode {
                    "  (Enter to finish search, Esc to clear)\r\n"
                } else {
                    "  ([/] to edit search)\r\n"
                }),
                ResetColor
            );
            lines += 1;
        } else {
            let _ = execute!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print(
                    "  (↑/↓ move • Enter select • [g] Scope: Dir/Global • [a] Filter Agent • [/] Search • Esc cancel)\r\n"
                ),
                ResetColor
            );
            lines += 1;
        }

        if filtered.is_empty() {
            let _ = execute!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print(
                    "    (No matching sessions found — press [g] for Global scope or [a] to switch agent)\r\n"
                ),
                ResetColor
            );
            lines += 1;
        } else {
            let start = if filtered.len() <= max_visible {
                0
            } else {
                idx.saturating_sub(max_visible / 2)
                    .min(filtered.len() - max_visible)
            };
            let end = (start + max_visible).min(filtered.len());

            for (i, s) in filtered.iter().enumerate().take(end).skip(start) {
                let title = title_cache
                    .get(&(s.agent, s.id.clone()))
                    .map(String::as_str)
                    .unwrap_or("(untitled)");
                let rel_time = s
                    .updated_at
                    .as_deref()
                    .map(format_relative_time)
                    .unwrap_or_else(|| "-".to_string());
                let dir_suffix = if global {
                    s.project_root
                        .as_deref()
                        .map(|p| format!(" · {}", short_project_name(p)))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let row = format!(
                    "[{}] {} — {} ({} ev{})",
                    s.agent.as_str(),
                    title,
                    rel_time,
                    s.event_count,
                    dir_suffix
                );
                if i == idx {
                    let _ = execute!(
                        stdout,
                        SetForegroundColor(Color::Green),
                        Print(format!("  ❯ {row}\r\n")),
                        ResetColor
                    );
                } else {
                    let _ = execute!(stdout, Print(format!("    {row}\r\n")));
                }
                lines += 1;
            }
            if filtered.len() > max_visible {
                let _ = execute!(
                    stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print(format!("    [{}/{}]\r\n", idx + 1, filtered.len())),
                    ResetColor
                );
                lines += 1;
            }
        }

        let _ = stdout.flush();
        drawn_lines = lines;

        match event::read().map_err(|e| e.to_string())? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                if search_mode {
                    match k.code {
                        KeyCode::Enter => {
                            search_mode = false;
                        }
                        KeyCode::Esc => {
                            search_query.clear();
                            search_mode = false;
                            idx = 0;
                        }
                        KeyCode::Backspace => {
                            search_query.pop();
                            idx = 0;
                        }
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            break Ok(None);
                        }
                        KeyCode::Char(c) => {
                            search_query.push(c);
                            idx = 0;
                        }
                        _ => {}
                    }
                    continue;
                }

                match k.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if !filtered.is_empty() {
                            idx = if idx == 0 {
                                filtered.len() - 1
                            } else {
                                idx - 1
                            };
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if !filtered.is_empty() {
                            idx = (idx + 1) % filtered.len();
                        }
                    }
                    KeyCode::Char('g') => {
                        global = !global;
                    }
                    KeyCode::Char('a') => {
                        agent_filter = match agent_filter {
                            None => Some(AgentId::OpenCode),
                            Some(AgentId::OpenCode) => Some(AgentId::AntigravityCli),
                            Some(AgentId::AntigravityCli) => Some(AgentId::Omp),
                            Some(AgentId::Omp) => None,
                        };
                        idx = 0;
                    }
                    KeyCode::Char('/') => {
                        search_mode = true;
                    }
                    KeyCode::Enter => {
                        if let Some(chosen) = filtered.get(idx) {
                            let title = title_cache
                                .get(&(chosen.agent, chosen.id.clone()))
                                .cloned()
                                .unwrap_or_else(|| chosen.id.clone());
                            break Ok(Some((chosen.agent, chosen.id.clone(), title)));
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        if !search_query.is_empty() {
                            search_query.clear();
                            idx = 0;
                        } else {
                            break Ok(None);
                        }
                    }
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        break Ok(None);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    };

    if drawn_lines > 0 {
        let _ = execute!(
            stdout,
            MoveUp(drawn_lines as u16),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        );
    }
    let _ = execute!(stdout, Show);
    let _ = disable_raw_mode();

    match res {
        Ok(Some((agent, id, title))) => {
            let _ = execute!(
                stdout,
                SetForegroundColor(Color::Green),
                Print("✔ "),
                ResetColor,
                Print(format!(
                    "Selected session › [{}] {}\n",
                    agent.as_str(),
                    title
                ))
            );
            Ok(Some((agent, id)))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn pick_target_agent(exclude: Option<AgentId>) -> Result<Option<AgentId>, String> {
    let candidates: Vec<AgentId> = AgentId::ALL
        .into_iter()
        .filter(|a| Some(*a) != exclude)
        .collect();
    let labels: Vec<String> = candidates
        .iter()
        .map(|a| match a {
            AgentId::OpenCode => "opencode".to_string(),
            AgentId::AntigravityCli => "antigravity-cli (agy)".to_string(),
            AgentId::Omp => "omp (Oh My Pi)".to_string(),
        })
        .collect();
    match select_menu("Select target agent to continue in", &labels, 0)? {
        Some(i) => Ok(Some(candidates[i])),
        None => Ok(None),
    }
}

/// Pick a previous handoff from history in a clean human-friendly list.
pub fn pick_handoff_history(store: &Path) -> Result<Option<String>, String> {
    let list = ho::list_handoffs(store).map_err(|e| e.to_string())?;
    if list.is_empty() {
        println!("No stored handoff packages found yet.");
        return Ok(None);
    }
    let items: Vec<String> = list
        .iter()
        .map(|m| {
            let rel = format_relative_time(&m.created_at);
            let proj = short_project_name(&m.project_root);
            let summary = ho::validate_handoff(store, &m.handoff_id, None)
                .ok()
                .map(|d| clean_snippet(&d.task_state.objective, 48))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| proj.clone());
            format!(
                "[{} → {}] {} — {} ({:?} · {})",
                m.source_agent.as_str(),
                m.target_agent.as_str(),
                summary,
                rel,
                m.confidence.overall,
                proj
            )
        })
        .collect();

    match select_menu("Select a previous handoff package", &items, 0)? {
        Some(i) => Ok(Some(list[i].handoff_id.clone())),
        None => Ok(None),
    }
}

/// Display compact handoff summary and present the 3 post-handoff actions:
/// 1. Copy continuation prompt to clipboard
/// 2. Print continuation prompt to screen
/// 3. Launch target agent CLI directly in project directory
pub fn handle_handoff_ready(detail: &ho::HandoffDetail, store_dir: &Path) -> Result<(), String> {
    let m = &detail.manifest;
    let v = &detail.validation;

    println!();
    println!(
        "  Handoff    : {} ({} → {})",
        m.handoff_id,
        m.source_agent.as_str(),
        m.target_agent.as_str()
    );
    println!("  Project    : {}", m.project_root);
    println!(
        "  Confidence : {:?} ({})",
        m.confidence.overall, m.confidence.reason
    );
    println!("  Preflight  : {:?}", v.status);
    for c in &v.conflicts {
        println!("  ! Conflict : {} — {}", c.id, c.description);
    }
    for w in &v.warnings {
        println!("  ! Notice   : {w}");
    }
    println!();

    if matches!(
        v.status,
        ho::PreflightStatus::Blocked | ho::PreflightStatus::Failed
    ) {
        return Err(format!(
            "Handoff {} blocked by preflight ({:?}). Resolve conflicts before continuing.",
            m.handoff_id, v.status
        ));
    }

    let pkg_dir = store_dir.join(&m.handoff_id);
    let prompt = ho::generate_continuation_prompt(detail, &pkg_dir);

    let actions = vec![
        format!(
            "Copy continuation prompt to clipboard (ready to paste in {})",
            m.target_agent.as_str()
        ),
        "Print continuation prompt to screen".to_string(),
        format!(
            "Launch {} directly in project directory (with prompt)",
            m.target_agent.as_str()
        ),
    ];

    let Some(action) = select_menu("Select next step", &actions, 0)? else {
        return Ok(());
    };

    match action {
        0 => {
            let method = copy_to_clipboard(&prompt);
            println!(
                "\n✓ Continuation prompt copied to clipboard ({method})!\n  Open `{}` and paste (Ctrl+V / Cmd+V).",
                m.target_agent.as_str()
            );
        }
        1 => {
            println!("\n{}", prompt.trim());
        }
        2 => {
            let _ = copy_to_clipboard(&prompt);
            launch_target_agent(m.target_agent, &m.project_root, &prompt)?;
        }
        _ => {}
    }

    Ok(())
}

/// Copy text to system clipboard using available platform tools + OSC-52 fallback.
pub fn copy_to_clipboard(text: &str) -> String {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
        ("clip.exe", &[]),
    ];

    for (bin, args) in candidates {
        if let Ok(mut child) = Command::new(bin)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if let Ok(status) = child.wait() {
                if status.success() {
                    emit_osc52(text);
                    return (*bin).to_string();
                }
            }
        }
    }

    emit_osc52(text);
    "OSC-52 terminal clipboard".to_string()
}

fn emit_osc52(text: &str) {
    let b64 = base64_encode(text.as_bytes());
    let mut stdout = io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{b64}\x07");
    let _ = stdout.flush();
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Launch the target agent CLI directly inside `project_root` with the continuation prompt.
pub fn launch_target_agent(
    target: AgentId,
    project_root: &str,
    prompt: &str,
) -> Result<(), String> {
    let dir = Path::new(project_root);
    let workdir = if dir.is_dir() { dir } else { Path::new(".") };

    println!(
        "\n🚀 Launching {} in {} (prompt also copied to clipboard)...",
        target.as_str(),
        workdir.display()
    );
    let _ = io::stdout().flush();

    let (bin, mut cmd) = match target {
        AgentId::OpenCode => {
            let mut c = Command::new("opencode");
            c.current_dir(workdir).arg("--prompt").arg(prompt).arg(".");
            ("opencode", c)
        }
        AgentId::AntigravityCli => {
            let mut c = Command::new("agy");
            c.current_dir(workdir).arg("-i").arg(prompt);
            ("agy", c)
        }
        AgentId::Omp => {
            let mut c = Command::new("omp");
            c.current_dir(workdir).arg(prompt);
            ("omp", c)
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(format!("failed to launch `{bin}`: {err}"))
    }

    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .map_err(|e| format!("failed to launch `{bin}`: {e}"))?;
        if !status.success() {
            return Err(format!(
                "`{}` exited with status: {status}",
                target.as_str()
            ));
        }
        Ok(())
    }
}

fn resolve_session_title(adapters: &[Box<dyn AgentAdapter>], s: &SessionSummary) -> String {
    if let Some(t) = s.title.as_deref() {
        let cleaned = clean_snippet(t, 56);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }
    for a in adapters {
        if a.id() == s.agent {
            if let Ok(full) = a.read_session(&s.id) {
                if let Some(first_user_msg) = full
                    .events
                    .iter()
                    .find(|ev| ev.actor == Actor::User && ev.kind == EventKind::UserMessage)
                    .and_then(|ev| ev.content.text.as_deref())
                {
                    let cleaned = clean_snippet(first_user_msg, 56);
                    if !cleaned.is_empty() {
                        return cleaned;
                    }
                }
            }
        }
    }
    format!("Session {}", &s.id[..s.id.len().min(8)])
}

fn clean_snippet(raw: &str, max_chars: usize) -> String {
    let mut text = raw.trim();
    if let Some((_, rest)) = text.split_once("<USER_REQUEST>") {
        text = rest.split("</USER_REQUEST>").next().unwrap_or(rest).trim();
    }
    let single_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = single_line.chars().count();
    if count <= max_chars {
        single_line
    } else {
        let truncated: String = single_line
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect();
        format!("{truncated}…")
    }
}

fn short_project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// Convert an ISO-8601 / RFC-3339 timestamp string into a concise relative time (`10m ago`, `2h ago`, `3d ago`).
pub fn format_relative_time(ts: &str) -> String {
    let Some(epoch_secs) = parse_iso8601_to_epoch(ts) else {
        return ts.chars().take(16).collect();
    };
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(epoch_secs);
    let diff = (now_secs - epoch_secs).max(0);
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 86_400 * 30 {
        format!("{}d ago", diff / 86_400)
    } else {
        ts.chars().take(10).collect()
    }
}

fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;

    let rest = &s[19..];
    let tz_offset_secs: i64 = if let Some(pos) = rest.rfind(['+', '-']) {
        let tz_str = &rest[pos..];
        let sign: i64 = if tz_str.starts_with('-') { -1 } else { 1 };
        let tz_h: i64 = tz_str.get(1..3)?.parse().ok()?;
        let tz_m: i64 = tz_str
            .get(4..6)
            .or_else(|| tz_str.get(3..5))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        sign * (tz_h * 3600 + tz_m * 60)
    } else {
        0
    };

    let days = days_from_civil(year, month, day);
    let total_secs = days * 86_400 + hour * 3600 + min * 60 + sec - tz_offset_secs;
    Some(total_secs)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_snippet_strips_user_request_and_truncates() {
        let raw = "  <USER_REQUEST>\nFix   login\nbug in auth.rs\n</USER_REQUEST> ";
        assert_eq!(clean_snippet(raw, 50), "Fix login bug in auth.rs");
        assert_eq!(clean_snippet(raw, 10), "Fix login…");
    }

    #[test]
    fn test_parse_iso8601_to_epoch() {
        let utc = parse_iso8601_to_epoch("2026-11-13T07:00:00Z").unwrap();
        let wib = parse_iso8601_to_epoch("2026-11-13T14:00:00+07:00").unwrap();
        assert_eq!(utc, wib);
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b"xm-relay"), "eG0tcmVsYXk=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }
}
