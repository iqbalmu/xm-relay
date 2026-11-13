use clap::{Parser, Subcommand};
use std::process::ExitCode;
use xm_relay_adapter_opencode::OpenCodeAdapter;
use xm_relay_core::{AgentAdapter, AgentId, SessionFilter};

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
    /// Discover and inspect agent sessions
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
}

#[derive(Subcommand)]
enum SessionsCommand {
    /// List sessions, newest first
    List {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show a normalised session and its parse report
    Inspect {
        session_id: String,
        #[arg(long, default_value = "opencode")]
        agent: String,
        #[arg(long)]
        json: bool,
    },
}

fn adapters() -> Vec<Box<dyn AgentAdapter>> {
    let mut v: Vec<Box<dyn AgentAdapter>> = Vec::new();
    if let Some(p) = OpenCodeAdapter::default_db_path() {
        v.push(Box::new(OpenCodeAdapter::new(p)));
    }
    v
}

fn find(agent: &str) -> Result<Box<dyn AgentAdapter>, String> {
    adapters()
        .into_iter()
        .find(|a| a.id().as_str() == agent)
        .ok_or_else(|| format!("no adapter for '{agent}' yet (implemented: opencode)"))
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Agents => {
            for id in AgentId::ALL {
                match adapters().into_iter().find(|a| a.id() == id) {
                    Some(a) => {
                        let i = a.detect().map_err(|e| e.to_string())?;
                        let state = if i.available { "found" } else { "not found" };
                        println!(
                            "{:<16} {state} version={} {}",
                            id.as_str(),
                            i.version.as_deref().unwrap_or("unknown"),
                            i.data_path
                                .map(|p| p.display().to_string())
                                .unwrap_or_default()
                        );
                    }
                    None => println!("{:<16} adapter not implemented", id.as_str()),
                }
            }
        }
        Command::Sessions {
            command: SessionsCommand::List { agent, limit },
        } => {
            let agent = agent.unwrap_or_else(|| "opencode".into());
            let list = find(&agent)?
                .list_sessions(&SessionFilter {
                    limit: Some(limit),
                    ..Default::default()
                })
                .map_err(|e| e.to_string())?;
            for s in list {
                println!(
                    "{}  {}  {:>5} msgs  {}  {}",
                    s.id,
                    s.updated_at.unwrap_or_default(),
                    s.event_count,
                    s.project_root.unwrap_or_default(),
                    s.title.unwrap_or_default()
                );
            }
        }
        Command::Sessions {
            command:
                SessionsCommand::Inspect {
                    session_id,
                    agent,
                    json,
                },
        } => {
            let s = find(&agent)?
                .read_session(&session_id)
                .map_err(|e| e.to_string())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?
                );
            } else {
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
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
