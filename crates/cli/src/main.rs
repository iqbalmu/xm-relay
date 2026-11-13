use clap::{Parser, Subcommand};
use std::process::ExitCode;
use xm_relay_adapter_antigravity::AntigravityAdapter;
use xm_relay_adapter_omp::OmpAdapter;
use xm_relay_adapter_opencode::OpenCodeAdapter;
use xm_relay_core::{AgentAdapter, AgentId, SessionFilter};
use xm_relay_repository as repo;
use xm_relay_security as sec;

#[derive(Parser)]
#[command(
    name = "xm-relay",
    version,
    about = "Cross-agent session handoff (read-only inspection)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Detect installed agents
    Agents,
    /// Inspect a project's Git state (read-only)
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Discover and inspect agent sessions
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Show branch, HEAD, working tree status and untracked files
    Inspect {
        /// Defaults to the current directory
        path: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Scan changes, untracked files and instructions for likely secrets (exit 2 if any)
    Scan {
        /// Defaults to the current directory
        path: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List project instruction files (AGENTS.md, CLAUDE.md, ...)
    Instructions {
        /// Defaults to the current directory
        path: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SessionsCommand {
    /// List sessions, newest first (all agents unless --agent is given)
    List {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show a normalised session and its parse report
    Inspect {
        session_id: String,
        /// Search all agents when omitted
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

fn adapters() -> Vec<Box<dyn AgentAdapter>> {
    let mut v: Vec<Box<dyn AgentAdapter>> = Vec::new();
    if let Some(p) = OpenCodeAdapter::default_db_path() {
        v.push(Box::new(OpenCodeAdapter::new(p)));
    }
    if let Some(p) = AntigravityAdapter::default_root() {
        v.push(Box::new(AntigravityAdapter::new(p)));
    }
    if let Some(p) = OmpAdapter::default_sessions_dir() {
        v.push(Box::new(OmpAdapter::new(p)));
    }
    v
}

fn select(agent: Option<String>) -> Result<Vec<Box<dyn AgentAdapter>>, String> {
    match agent {
        None => Ok(adapters()),
        Some(name) => {
            let found: Vec<_> = adapters()
                .into_iter()
                .filter(|a| a.id().as_str() == name)
                .collect();
            if found.is_empty() {
                Err(format!(
                    "no adapter for '{name}' yet (implemented: opencode, antigravity-cli, omp)"
                ))
            } else {
                Ok(found)
            }
        }
    }
}

fn agents() -> Result<(), String> {
    for id in AgentId::ALL {
        match adapters().into_iter().find(|a| a.id() == id) {
            Some(a) => {
                let i = a.detect().map_err(|e| e.to_string())?;
                println!(
                    "{:<16} {} version={} {}",
                    id.as_str(),
                    if i.available { "found" } else { "not found" },
                    i.version.as_deref().unwrap_or("unknown"),
                    i.data_path
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
            }
            None => println!("{:<16} adapter not implemented", id.as_str()),
        }
    }
    Ok(())
}

fn list(agent: Option<String>, limit: usize) -> Result<(), String> {
    let filter = SessionFilter {
        limit: Some(limit),
        ..Default::default()
    };
    let mut all = Vec::new();
    for a in select(agent)? {
        match a.list_sessions(&filter) {
            Ok(l) => all.extend(l),
            Err(e) => eprintln!("warning: {}: {e}", a.id().as_str()),
        }
    }
    all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    all.truncate(limit);
    for s in all {
        println!(
            "{:<15} {}  {}  {:>5} ev  {}  {}",
            s.agent.as_str(),
            s.id,
            s.updated_at.unwrap_or_default(),
            s.event_count,
            s.project_root.unwrap_or_default(),
            s.title.unwrap_or_default()
        );
    }
    Ok(())
}

fn inspect(id: &str, agent: Option<String>, json: bool) -> Result<(), String> {
    let s = select(agent)?
        .iter()
        .find_map(|a| a.read_session(id).ok())
        .ok_or_else(|| format!("session '{id}' not found"))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let r = &s.parse_report;
    println!(
        "session   {}\nagent     {} (version {})",
        s.id,
        s.agent.id.as_str(),
        s.agent.version.as_deref().unwrap_or("unknown")
    );
    println!(
        "project   {}",
        s.project
            .as_ref()
            .map(|p| p.root.as_str())
            .unwrap_or("unknown")
    );
    println!("events    {} normalised", s.events.len());
    println!(
        "parse     total={} parsed={} unsupported={} errors={}",
        r.total_events,
        r.parsed_events,
        r.unsupported_events(),
        r.errors.len()
    );
    for u in &r.unsupported {
        println!(
            "  ! unsupported at {}: {:?} ({})",
            u.position, u.native_kind, u.reason
        );
    }
    for e in &r.errors {
        println!("  ! error: {e}");
    }
    if r.is_lossy() {
        println!("WARNING: parse is lossy; handoff confidence must be lowered");
    }
    Ok(())
}

fn project_inspect(path: Option<std::path::PathBuf>, json: bool) -> Result<(), String> {
    let path = path.unwrap_or_else(|| std::path::PathBuf::from("."));
    let st = repo::inspect(&path).map_err(|e| e.to_string())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&st).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if !st.is_repository {
        println!("{} is not inside a Git repository", path.display());
        return Ok(());
    }
    let branch = match (&st.branch, st.detached) {
        (Some(b), _) => b.clone(),
        (None, true) => "(detached HEAD)".into(),
        _ => "unknown".into(),
    };
    println!("root      {}", st.root.as_deref().unwrap_or(""));
    println!(
        "branch    {branch}{}",
        st.upstream
            .as_ref()
            .map(|u| format!(
                " -> {u} (ahead {}, behind {})",
                st.ahead.unwrap_or(0),
                st.behind.unwrap_or(0)
            ))
            .unwrap_or_default()
    );
    println!(
        "head      {}",
        st.head_commit.as_deref().unwrap_or("(no commits)")
    );
    println!(
        "state     {}",
        if st.working_tree_dirty {
            "DIRTY"
        } else {
            "clean"
        }
    );
    println!(
        "staged    {}   unstaged {}   untracked {}   conflicted {}",
        st.staged.len(),
        st.unstaged.len(),
        st.untracked.len(),
        st.conflicted.len()
    );
    let d = &st.diff_summary;
    println!(
        "diff      {} files, +{} -{} ({} binary)",
        d.files.len(),
        d.additions,
        d.deletions,
        d.binary_files
    );
    for c in &st.conflicted {
        println!("  ! conflict: {c}");
    }
    for w in &st.warnings {
        println!("  ! {w}");
    }
    if st.worktrees.len() > 1 {
        println!("worktrees {}", st.worktrees.len());
    }
    for c in st.recent_commits.iter().take(3) {
        println!("  {} {}", &c.hash[..c.hash.len().min(8)], c.subject);
    }
    Ok(())
}

fn project_instructions(path: Option<std::path::PathBuf>, json: bool) -> Result<(), String> {
    let set = repo::discover_instructions(&path.unwrap_or_else(|| std::path::PathBuf::from(".")));
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&set).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    println!("root      {}", set.root);
    if set.files.is_empty() {
        println!("no instruction files found");
    }
    for f in &set.files {
        println!(
            "{:<8} {}{}  {} bytes  sha256 {}",
            format!("{:?}", f.kind).to_lowercase(),
            f.path,
            if f.applies_to_start {
                ""
            } else {
                " (other scope)"
            },
            f.size,
            f.sha256.as_deref().map(|h| &h[..12]).unwrap_or("-")
        );
    }
    for w in &set.warnings {
        println!("  ! {w}");
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct ScanHit {
    location: String,
    what: String,
    line: Option<usize>,
    fingerprint: Option<String>,
}

/// Best-effort and read-only. Reports locations and kinds, never secret values.
fn project_scan(path: Option<std::path::PathBuf>, json: bool) -> Result<bool, String> {
    let path = path.unwrap_or_else(|| std::path::PathBuf::from("."));
    let st = repo::inspect(&path).map_err(|e| e.to_string())?;
    let root = std::path::PathBuf::from(st.root.clone().ok_or("not inside a Git repository")?);
    let mut hits: Vec<ScanHit> = Vec::new();
    let mut scanned = 0usize;

    let mut names: Vec<&str> = st.untracked.iter().map(|f| f.path.as_str()).collect();
    names.extend(
        st.staged
            .iter()
            .chain(&st.unstaged)
            .map(|f| f.path.as_str()),
    );
    names.sort();
    names.dedup();
    for n in names {
        if let Some(s) = sec::sensitive_file(n) {
            hits.push(ScanHit {
                location: n.into(),
                what: format!("sensitive file name ({})", s.reason),
                line: None,
                fingerprint: None,
            });
        }
    }
    for f in &st.untracked {
        let Some(size) = f.size else { continue };
        if f.is_symlink || size > 5 * 1024 * 1024 {
            continue;
        }
        let Ok(bytes) = std::fs::read(root.join(&f.path)) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        scanned += 1;
        for x in sec::scan_text(&text) {
            hits.push(ScanHit {
                location: f.path.clone(),
                what: x.kind.as_str().into(),
                line: Some(x.line),
                fingerprint: Some(x.fingerprint),
            });
        }
    }
    let patch = repo::tracked_patch(&root).map_err(|e| e.to_string())?;
    scanned += 1;
    for x in sec::scan_patch(&String::from_utf8_lossy(&patch)) {
        hits.push(ScanHit {
            location: format!("{} (added lines in tracked diff)", x.path),
            what: x.finding.kind.as_str().into(),
            line: None,
            fingerprint: Some(x.finding.fingerprint),
        });
    }
    for f in repo::discover_instructions(&root).files {
        if let Some(text) = &f.content {
            scanned += 1;
            for x in sec::scan_text(text) {
                hits.push(ScanHit {
                    location: f.path.clone(),
                    what: x.kind.as_str().into(),
                    line: Some(x.line),
                    fingerprint: Some(x.fingerprint),
                });
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&hits).map_err(|e| e.to_string())?
        );
    } else {
        for h in &hits {
            let line = h.line.map(|l| format!(":{l}")).unwrap_or_default();
            println!(
                "{}{line}  {}{}",
                h.location,
                h.what,
                h.fingerprint
                    .as_ref()
                    .map(|f| format!("  [{f}]"))
                    .unwrap_or_default()
            );
        }
        println!("{} finding(s) in {} scanned source(s). Heuristic scan: absence of findings is not proof of safety.", hits.len(), scanned);
    }
    Ok(!hits.is_empty())
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Agents => agents(),
        Command::Project {
            command: ProjectCommand::Inspect { path, json },
        } => project_inspect(path, json),
        Command::Project {
            command: ProjectCommand::Scan { path, json },
        } => {
            return match project_scan(path, json) {
                Ok(false) => ExitCode::SUCCESS,
                Ok(true) => ExitCode::from(2),
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Project {
            command: ProjectCommand::Instructions { path, json },
        } => project_instructions(path, json),
        Command::Sessions {
            command: SessionsCommand::List { agent, limit },
        } => list(agent, limit),
        Command::Sessions {
            command:
                SessionsCommand::Inspect {
                    session_id,
                    agent,
                    json,
                },
        } => inspect(&session_id, agent, json),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
