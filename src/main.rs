//! Ribbon CLI — command-line interface for the agent communication ribbon.

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand};
use ribbon::{
    agent_statuses, append_event, find_previous_state, read_events, render, state_machine,
    verify_events, verify_report, DiscoveredConfig, EventFilter, EventType, GitRoots, RenderFormat,
    RenderOpts, RibbonConfig, RibbonEvent, ScopeConfig,
};
use std::path::PathBuf;

/// Ribbon — agent communication via structured ndjson event log.
///
/// A file-first, git-versioned protocol for coordinating asynchronous agents.
/// Compact ndjson for machines, rich markdown for humans.
///
/// # Quick Start
///
///   ribbon init --git-root myagent=.
///   ribbon send working --agent myagent --task "add feature"
///   ribbon send completed --agent myagent --task "add feature" --commit abc123
///   ribbon status
///   ribbon verify
///
/// # How it works
///
///   Agents append structured events to an ndjson log (events.ndjson).
///   Ribbon queries, renders, and verifies those events.
///   The log is git-versioned — every event is auditable.
///   Configure git roots in .ribbon/config.toml to enable 'ribbon verify'.
#[derive(Parser)]
#[command(name = "ribbon", version, about, long_about = None, after_help = "Docs: https://github.com/allen-munsch/ribbon")]
struct Cli {
    /// Path to the ndjson event log (overrides config)
    #[arg(short = 'L', long, env = "RIBBON_LOG_PATH", global = true)]
    log: Option<PathBuf>,

    /// Path to ribbon config file
    #[arg(short = 'c', long, env = "RIBBON_CONFIG", global = true)]
    config: Option<PathBuf>,

    /// Project root directory — where to search for .ribbon/config.toml
    /// Default: walks up from current directory
    #[arg(
        short = 'r',
        long = "project-root",
        env = "RIBBON_PROJECT_ROOT",
        global = true
    )]
    project_root: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize ribbon in the current directory
    #[command(
        long_about = "Initialize ribbon in the current directory.\n\nCreates .ribbon/config.toml with default settings.\nUse --git-root to pre-configure git verification paths so\n'ribbon verify' works immediately.\n\nExamples:\n  ribbon init\n  ribbon init --git-root frontend=.\n  ribbon init --git-root backend=../services/api --git-root infra=."
    )]
    Init(InitArgs),

    /// Send (append) an event to the log
    Send(SendArgs),

    /// Show current status of all agents
    Status(StatusArgs),

    /// Query events with filters
    Query(QueryArgs),

    /// Render events as human-readable markdown/text
    Render(RenderArgs),

    /// Watch for new events in real time
    Watch(WatchArgs),

    /// Verify commit hashes against git origins
    #[command(
        long_about = "Verify that commit hashes in the event log exist on git origins.\n\nChecks every 'committed' and 'completed' event against the\nconfigured git remotes. Prevents agents from claiming work\nthat was never pushed.\n\nPREREQUISITES:\n  .ribbon/config.toml must have [git_roots] mapping agent names\n  to git repository paths. Create one with 'ribbon init', then\n  edit the file, or use 'ribbon init --git-root agent=path'.\n\nExit code: 0 if all commits verified, 1 if any are missing.\n\nExamples:\n  ribbon verify                  # Verify all agents\n  ribbon verify --agent mosaic   # Verify only one agent\n  ribbon verify --json           # Machine-readable output"
    )]
    Verify(VerifyArgs),

    /// Discover your agent identity from scope.toml
    #[command(
        long_about = "Discover your agent identity by matching your current directory\nagainst the scope paths defined in .ribbon/scope.toml.\n\nUse this when you spawn in a submodule and need to know:\n- Which agent you are\n- What paths you own\n- What services you manage\n- Who your peer agents are\n\nExamples:\n  ribbon whoami                  # From current directory\n  ribbon whoami --cwd submodules/mosaic\n  ribbon whoami --json"
    )]
    Whoami(WhoamiArgs),

    /// Show scope information for yourself or another agent
    #[command(
        long_about = "Show what an agent owns: paths, services, git root, and peers.\nWithout --agent, acts like 'whoami' for your current directory.\n\nExamples:\n  ribbon scope                   # My scope (from cwd)\n  ribbon scope --agent mosaic    # Mosaic's scope\n  ribbon scope --agent weft --json"
    )]
    Scope(ScopeArgs),

    /// Print the working directory this agent should cd into
    #[command(
        long_about = "Print the directory where this agent should work.\n\nResolves your agent identity from .ribbon/scope.toml and prints the\ngit root path (or first owned path). Pipe into cd:\n\n  cd $(ribbon cd)\n\nUse this at the start of every agent session to ensure you're in the\nright directory with the right events.ndjson."
    )]
    Cd(CdArgs),

    /// Pack the event log (Brotli compress)
    Pack(PackArgs),

    /// Unpack a compressed event log
    Unpack(UnpackArgs),
}

// ── send ──────────────────────────────────────────────────────────────

#[derive(Parser)]
struct SendArgs {
    /// Event type: submitted, working, committed, completed, failed, note,
    /// blocked, confused, sudo, sudo_granted, sudo_denied, hitl, hitl_resolved
    #[arg(value_name = "EVENT")]
    event: String,

    /// Agent name/identifier
    #[arg(short, long)]
    agent: String,

    /// Task description or identifier
    #[arg(short, long)]
    task: Option<String>,

    /// Git commit hash (required for committed/completed)
    #[arg(short = 'H', long, value_name = "SHA")]
    commit: Option<String>,

    /// Human-readable message
    #[arg(short, long)]
    msg: Option<String>,

    /// Test count (for completed events)
    #[arg(long)]
    tests: Option<u32>,

    /// Test failure count (default 0)
    #[arg(long, default_value = "0")]
    failures: u32,

    /// Priority: LOW, MEDIUM, HIGH, CRITICAL
    #[arg(short, long)]
    priority: Option<String>,

    /// Whether this task blocks downstream work
    #[arg(short = 'B', long, action = ArgAction::SetTrue, default_value = "false")]
    blocking: bool,

    /// DANGER: Bypass state machine validation. This orphans the task —
    /// state transitions will break and the task will appear stuck.
    /// Only use when you have exhausted all other options and understand
    /// that the event log will be inconsistent.
    #[arg(short = 'F', long, action = ArgAction::SetTrue, default_value = "false", hide_short_help = true)]
    force: bool,
}

fn cmd_send(args: SendArgs, log_path: &std::path::Path, config: &RibbonConfig) -> Result<()> {
    // ── Agent name validation ──────────────────────────────────────────────
    // Check --agent against the config.agents whitelist (if non-empty).
    // This catches typos (yas-mcp vs yas_mcp), prevents orphan tasks filed
    // under non-existent agent names, and enforces naming conventions.
    if !config.agents.is_empty() && !config.agents.contains(&args.agent) {
        let valid = config
            .agents
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "Unknown agent \"{}\".\n\n  Valid agents (from .ribbon/config.toml): {valid}\n\n  HINT: Did you mean one of these? Check your spelling.\n  HINT: Run `ribbon whoami` to auto-discover your agent name from your current directory.\n  HINT: To add a new agent, edit .ribbon/config.toml and add it to the agents list.\n  HINT: Use `ribbon status` to see all known agents and their current state.",
            args.agent
        );
    }

    let event_type: EventType = serde_json::from_str(&format!("\"{}\"", args.event)).context(
        "Invalid event type. Use: submitted, working, committed, completed, failed, note, blocked, confused, sudo, sudo_granted, sudo_denied, hitl, hitl_resolved",
    )?;

    // ── State machine validation ──────────────────────────────────────────
    // Track the canonical task string (from the matched event) for consistency
    let mut canonical_task: Option<String> = None;
    // Track previous state so we can detect self-loops (task amendment)
    let mut prev_state: Option<EventType> = None;

    if !args.force && event_type != EventType::Note {
        // Load existing events to find previous state for this agent+task
        let events = match read_events(log_path) {
            Ok(e) => e,
            Err(ribbon::StoreError::NotFound(_)) => vec![],
            Err(e) => return Err(e.into()),
        };

        let (ps, matched_task) = if let Some(ref task) = args.task {
            match find_previous_state(&events, &args.agent, task) {
                Some((et, canonical)) => (Some(et), canonical),
                None => (None, None),
            }
        } else {
            (None, None)
        };

        prev_state = ps;
        canonical_task = matched_task;

        let sm = state_machine();
        match sm.validate(prev_state.as_ref(), &event_type) {
            Ok(breadcrumb) => {
                eprintln!("  {}", breadcrumb);
            }
            Err(hints) => {
                anyhow::bail!("{hints}");
            }
        }
    } else if args.force && event_type != EventType::Note {
        eprintln!("  ⚠️  STATE MACHINE BYPASSED. The task is now orphaned — subsequent");
        eprintln!("  ⚠️  state transitions will likely fail. Fix the root cause instead.");
    }

    let emoji = event_type.emoji();
    let label = event_type.label();

    let mut event = RibbonEvent::new(&args.agent, event_type.clone());

    // Normalize task: for self-loops (e.g. submitted→submitted to amend),
    // use the new task text; otherwise preserve the canonical task for consistency
    let task_str = if prev_state.as_ref() == Some(&event_type) {
        // Self-loop: agent is amending — use the new task text
        args.task.map(|t| t.trim().to_string()).or(canonical_task)
    } else {
        canonical_task.or(args.task.map(|t| t.trim().to_string()))
    };
    if let Some(task) = task_str {
        if task.is_empty() {
            anyhow::bail!("Task cannot be empty (after trimming whitespace).");
        }
        event = event.with_task(task);
    }
    if let Some(commit) = args.commit {
        event = event.with_commit(commit);
    }
    if let Some(msg) = args.msg {
        event = event.with_msg(msg);
    }
    if let Some(tests) = args.tests {
        event = event.with_tests(tests, args.failures);
    }
    if let Some(priority) = args.priority {
        event = event.with_priority(priority);
    }
    if args.blocking {
        event = event.with_blocking(true);
    }

    append_event(log_path, &event)?;
    eprintln!("{emoji} {label} {}", event.agent);
    Ok(())
}

// ── status ────────────────────────────────────────────────────────────

#[derive(Parser)]
struct StatusArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn cmd_status(args: StatusArgs, log_path: &std::path::Path) -> Result<()> {
    let events = match read_events(log_path) {
        Ok(e) => e,
        Err(ribbon::StoreError::NotFound(_)) => {
            if args.json {
                println!("[]");
            } else {
                println!("No events yet. Use 'ribbon send' to add events.");
            }
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let statuses = agent_statuses(&events);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
    } else {
        if statuses.is_empty() {
            println!("No agents found.");
            return Ok(());
        }

        // Show project root so agents know their context
        if let Ok(cwd) = std::env::current_dir() {
            println!("project: {}", cwd.display());
        }
        println!();
        println!(
            "{:<16} {:<12} {:<10} {:<6} {:<6} ACTIVE TASK",
            "AGENT", "STATE", "COMMIT", "DONE", "FAIL"
        );
        println!("{}", "-".repeat(80));

        for s in &statuses {
            let commit = s
                .last_commit
                .as_ref()
                .map(|c| &c[..c.len().min(7)])
                .unwrap_or("-");
            let state_icon = match s.state.as_str() {
                "working" | "submitted" | "committed" => "🟡",
                "completed" => "✅",
                "failed" => "❌",
                "blocked" => "🚫",
                "confused" => "❓",
                "sudo_pending" => "🔐",
                "hitl_pending" => "👤",
                _ => "⚪",
            };
            println!(
                "{:<16} {} {:<12} {:<7} {:<6} {:<6} {}",
                s.agent,
                state_icon,
                s.state,
                commit,
                s.completed_count,
                s.failed_count,
                s.active_task.as_deref().unwrap_or("-"),
            );
            // Surface coordination details
            if let Some(ref msg) = s.blocked_by {
                println!("  BLOCKED: {}", msg);
            }
            if let Some(ref msg) = s.confused_about {
                println!("  CONFUSED: {}", msg);
            }
            if let Some(ref msg) = s.sudo_request {
                println!("  SUDO: {}", msg);
            }
            if let Some(ref msg) = s.hitl_request {
                println!("  HITL: {}", msg);
            }
            // Show what the agent can do next
            if !s.next_actions.is_empty() {
                let task_hint = s.active_task.as_deref().unwrap_or("...");
                let task_short: String = if task_hint.len() > 60 {
                    format!("{}...", &task_hint[..57])
                } else {
                    task_hint.to_string()
                };
                for action in &s.next_actions {
                    let req = if action == "committed" {
                        " --commit <SHA>"
                    } else if action == "completed" {
                        " --tests N --failures 0"
                    } else {
                        ""
                    };
                    println!(
                        "  → ribbon send {} --agent {} --task \"{}\"{}",
                        action, s.agent, task_short, req
                    );
                }
            }
        }
    }
    Ok(())
}

// ── query ─────────────────────────────────────────────────────────────

#[derive(Parser)]
struct QueryArgs {
    /// Filter by agent
    #[arg(short, long)]
    agent: Option<String>,

    /// Filter by event type
    #[arg(short, long)]
    event: Option<String>,

    /// Filter since ISO timestamp (e.g. 2026-05-11T00:00:00Z)
    #[arg(long)]
    since: Option<String>,

    /// Filter by task substring
    #[arg(long)]
    task: Option<String>,

    /// Limit to last N events
    #[arg(short, long)]
    limit: Option<usize>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn cmd_query(args: QueryArgs, log_path: &std::path::Path) -> Result<()> {
    let events = read_events(log_path)?;
    let mut filter = EventFilter::all();

    if let Some(agent) = args.agent {
        filter = filter.agent(agent);
    }
    if let Some(event_str) = args.event {
        let et: EventType =
            serde_json::from_str(&format!("\"{event_str}\"")).context("Invalid event type")?;
        filter = filter.event_type(et);
    }
    if let Some(since_str) = args.since {
        let ts = chrono::DateTime::parse_from_rfc3339(&since_str)
            .context("Invalid timestamp. Use ISO 8601 format (e.g. 2026-05-11T00:00:00Z)")?;
        filter = filter.since(ts.with_timezone(&chrono::Utc));
    }
    if let Some(task) = args.task {
        filter = filter.task_contains(task);
    }
    if let Some(limit) = args.limit {
        filter = filter.limit(limit);
    }

    let results = filter.apply(events.iter());

    if args.json {
        let events: Vec<&ribbon::RibbonEvent> = results.into_iter().collect();
        println!("{}", serde_json::to_string_pretty(&events)?);
    } else {
        for event in &results {
            println!("{}", event.to_ndjson_line()?);
        }
    }
    Ok(())
}

// ── render ────────────────────────────────────────────────────────────

#[derive(Parser)]
struct RenderArgs {
    /// Output format: markdown, plain, compact
    #[arg(short, long, default_value = "markdown")]
    format: String,

    /// Only render events since this ISO date (e.g. 2026-05-11)
    #[arg(long)]
    since: Option<String>,

    /// Only render events for this agent
    #[arg(short, long)]
    agent: Option<String>,

    /// Group events by agent
    #[arg(short = 'G', long)]
    group_by_agent: bool,

    /// Disable emoji
    #[arg(long)]
    no_emoji: bool,
}

fn cmd_render(args: RenderArgs, log_path: &std::path::Path) -> Result<()> {
    let events = read_events(log_path)?;
    let format = match args.format.as_str() {
        "markdown" | "md" => RenderFormat::Markdown,
        "plain" | "text" | "txt" => RenderFormat::Plain,
        "compact" | "short" => RenderFormat::Compact,
        other => anyhow::bail!("Unknown format: {other}. Use: markdown, plain, compact"),
    };

    let opts = RenderOpts {
        format,
        since: args.since,
        agent: args.agent,
        group_by_agent: args.group_by_agent,
        emoji: !args.no_emoji,
    };

    let output = render(&events, &opts);
    print!("{output}");
    Ok(())
}

// ── watch ─────────────────────────────────────────────────────────────

#[derive(Parser)]
struct WatchArgs {
    /// Output format: json, compact
    #[arg(short, long, default_value = "compact")]
    format: String,
}

fn cmd_watch(args: WatchArgs, log_path: &std::path::Path) -> Result<()> {
    // Simple polling-based watch that works without the notify dependency
    let log_path = log_path.to_path_buf();

    // Get initial file size
    let mut last_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

    eprintln!("Watching {} (Ctrl+C to stop)", log_path.display());

    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));

        let current_size = match std::fs::metadata(&log_path) {
            Ok(m) => m.len(),
            Err(_) => {
                // File might not exist yet
                continue;
            }
        };

        if current_size > last_size {
            // Read new content
            if let Ok(events) = read_events(&log_path) {
                let new_events: Vec<_> = if !events.is_empty() {
                    // Estimate which events are new based on size increase
                    // For simplicity, just show the last few
                    let show = events.len().saturating_sub(3);
                    events[show..].to_vec()
                } else {
                    vec![]
                };

                for event in &new_events {
                    match args.format.as_str() {
                        "json" => println!("{}", event.to_ndjson_line()?),
                        _ => {
                            let opts = RenderOpts {
                                format: RenderFormat::Compact,
                                emoji: true,
                                ..Default::default()
                            };
                            let line = render(std::slice::from_ref(event), &opts);
                            print!("{line}");
                        }
                    }
                }
            }
            last_size = current_size;
        }
    }
}

// ── verify ────────────────────────────────────────────────────────────

#[derive(Parser)]
struct VerifyArgs {
    /// Only verify this agent
    #[arg(short, long)]
    agent: Option<String>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn cmd_verify(args: VerifyArgs, log_path: &std::path::Path, config: &RibbonConfig) -> Result<()> {
    let events = read_events(log_path)?;

    // Build git roots from config
    let mut git_roots = GitRoots::default()
        .with_remote(&config.git_remote)
        .with_branch(&config.git_branch);

    for (agent, path) in &config.git_roots {
        git_roots = git_roots.with_agent(agent, path);
    }

    let mut results = verify_events(&events, &git_roots);

    // Filter by agent if requested
    if let Some(ref agent_filter) = args.agent {
        results.retain(|r| &r.agent == agent_filter);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        println!("{}", verify_report(&results));
    }

    // Exit with non-zero if any unverified commits
    let all_ok = results.iter().all(|r| r.on_origin);
    if !all_ok {
        std::process::exit(1);
    }

    Ok(())
}

// ── whoami / scope ────────────────────────────────────────────────────

#[derive(Parser)]
struct WhoamiArgs {
    /// Working directory to resolve (default: current directory)
    #[arg(long, env = "PWD")]
    cwd: Option<PathBuf>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct CdArgs {
    /// Agent name (if auto-detection doesn't work from cwd)
    #[arg(short, long)]
    agent: Option<String>,

    /// Working directory to resolve from (default: current directory)
    #[arg(long, env = "PWD")]
    cwd: Option<PathBuf>,
}

fn cmd_cd(args: CdArgs, config: &RibbonConfig) -> Result<()> {
    let cwd = args
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let (scope_config, project_root) = match ScopeConfig::discover_from(Some(&cwd))? {
        Some((sc, pr)) => (sc, pr),
        None => {
            anyhow::bail!("No .ribbon/scope.toml found. Create one to define agent scopes.");
        }
    };

    // If --agent specified, use that directly
    let target = if let Some(ref agent_name) = args.agent {
        match scope_config.agents.get(agent_name) {
            Some(entry) => {
                let git_root = config.git_roots.get(agent_name).cloned();
                let first_path = entry.paths.first().map(|p| {
                    let base = p.trim_end_matches('/');
                    let base = if let Some(pos) = base.find('*') {
                        &base[..pos]
                    } else {
                        base
                    };
                    project_root.join(base)
                });
                git_root
                    .or(first_path)
                    .unwrap_or_else(|| project_root.clone())
            }
            None => anyhow::bail!(
                "Unknown agent: {agent_name}. Known: {}",
                scope_config
                    .agents
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    } else {
        // Auto-detect from cwd
        match scope_config.whoami(&cwd, &project_root, config) {
            Some(result) => result.git_root.unwrap_or_else(|| {
                result
                    .paths
                    .first()
                    .map(|p| {
                        let base = p.trim_end_matches('/');
                        let base = if let Some(pos) = base.find('*') {
                            &base[..pos]
                        } else {
                            base
                        };
                        project_root.join(base)
                    })
                    .unwrap_or_else(|| project_root.clone())
            }),
            None => {
                // List all agents and their paths so the agent can pick
                eprintln!("Could not auto-detect agent from: {}", cwd.display());
                eprintln!();
                eprintln!("Available agents:");
                for (name, entry) in &scope_config.agents {
                    let path = config.git_roots.get(name).cloned().or_else(|| {
                        entry.paths.first().map(|p| {
                            let base = p.trim_end_matches('/');
                            let base = if let Some(pos) = base.find('*') {
                                &base[..pos]
                            } else {
                                base
                            };
                            project_root.join(base)
                        })
                    });
                    eprintln!(
                        "  ribbon cd --agent {}{}",
                        name,
                        path.map(|p| format!("  # → {}", p.display()))
                            .unwrap_or_default()
                    );
                }
                std::process::exit(1);
            }
        }
    };

    println!("{}", target.display());
    Ok(())
}

#[derive(Parser)]
struct ScopeArgs {
    /// Agent to show scope for (default: auto-detect from cwd)
    #[arg(short, long)]
    agent: Option<String>,

    /// Working directory (for auto-detection when --agent not given)
    #[arg(long, env = "PWD")]
    cwd: Option<PathBuf>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn cmd_whoami(args: WhoamiArgs, config: &RibbonConfig) -> Result<()> {
    let cwd = args
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Discover scope.toml from project root
    let (scope_config, project_root) = match ScopeConfig::discover_from(Some(&cwd))? {
        Some((sc, pr)) => (sc, pr),
        None => {
            anyhow::bail!(
                "No .ribbon/scope.toml found.\n\n  HINT: Create one at <project>/.ribbon/scope.toml to define agent scopes.\n  HINT: Use `ribbon init` first if you haven't set up ribbon yet."
            );
        }
    };

    match scope_config.whoami(&cwd, &project_root, config) {
        Some(result) => {
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "agent": result.agent,
                        "paths": result.paths,
                        "docker_services": result.docker_services,
                        "git_root": result.git_root.as_ref().map(|p| p.display().to_string()),
                        "project_root": result.project_root.display().to_string(),
                        "cwd": cwd.display().to_string(),
                        "peers": result.peers.iter().map(|(name, paths)| {
                            serde_json::json!({"agent": name, "paths": paths})
                        }).collect::<Vec<_>>(),
                    }))?
                );
            } else {
                println!("Agent:     {}", result.agent);
                println!("Scope:     {}", result.paths.join(", "));
                if !result.docker_services.is_empty() {
                    println!("Services:  {}", result.docker_services.join(", "));
                }
                if let Some(ref git_root) = result.git_root {
                    println!("Git root:  {}", git_root.display());
                }
                println!("Project:   {}", result.project_root.display());
                println!("CWD:       {}", cwd.display());
                if !result.peers.is_empty() {
                    println!();
                    println!("Peer agents:");
                    for (name, paths) in &result.peers {
                        println!("  {name}: {}", paths.join(", "));
                    }
                }
            }
        }
        None => {
            // No scope matched — list all known agents
            eprintln!("Could not determine agent identity for: {}", cwd.display());
            eprintln!();
            if scope_config.agents.is_empty() {
                eprintln!("No agents defined in .ribbon/scope.toml.");
            } else {
                eprintln!("Known agents in .ribbon/scope.toml:");
                for (name, entry) in &scope_config.agents {
                    eprintln!("  {name}: {}", entry.paths.join(", "));
                }
                eprintln!();
                eprintln!("HINT: Are you in the right directory? Your cwd must be inside");
                eprintln!("one of the scope paths listed above.");
            }
            std::process::exit(1);
        }
    }

    Ok(())
}

fn cmd_scope(args: ScopeArgs, config: &RibbonConfig) -> Result<()> {
    let (scope_config, project_root) = {
        let search_root = args.cwd.as_deref();
        match ScopeConfig::discover_from(search_root)? {
            Some((sc, pr)) => (sc, pr),
            None => {
                anyhow::bail!("No .ribbon/scope.toml found. Create one to define agent scopes.")
            }
        }
    };

    // If --agent specified, show that agent's scope
    if let Some(ref agent_name) = args.agent {
        match scope_config.agents.get(agent_name) {
            Some(entry) => {
                let git_root = config.git_roots.get(agent_name).cloned();
                let peers: Vec<_> = scope_config
                    .agents
                    .iter()
                    .filter(|(a, _)| *a != agent_name)
                    .collect();

                if args.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "agent": agent_name,
                            "paths": entry.paths,
                            "docker_services": entry.docker_services,
                            "git_root": git_root.as_ref().map(|p| p.display().to_string()),
                            "project_root": project_root.display().to_string(),
                            "peers": peers.iter().map(|(name, e)| {
                                serde_json::json!({"agent": name, "paths": e.paths})
                            }).collect::<Vec<_>>(),
                        }))?
                    );
                } else {
                    println!("Agent:     {agent_name}");
                    println!("Scope:     {}", entry.paths.join(", "));
                    if !entry.docker_services.is_empty() {
                        println!("Services:  {}", entry.docker_services.join(", "));
                    }
                    if let Some(ref gr) = git_root {
                        println!("Git root:  {}", gr.display());
                    }
                    println!("Project:   {}", project_root.display());
                    if !peers.is_empty() {
                        println!();
                        println!("Peer agents:");
                        for (name, entry) in &peers {
                            println!("  {name}: {}", entry.paths.join(", "));
                        }
                    }
                }
            }
            None => {
                anyhow::bail!(
                    "Unknown agent: {agent_name}.\n\n  Known agents: {}",
                    scope_config
                        .agents
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    } else {
        // No --agent: resolve from cwd (same as whoami)
        let cwd = args
            .cwd
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        match scope_config.whoami(&cwd, &project_root, config) {
            Some(_) => {
                let wargs = WhoamiArgs {
                    cwd: Some(cwd),
                    json: args.json,
                };
                return cmd_whoami(wargs, config);
            }
            None => {
                anyhow::bail!(
                    "Could not determine agent identity for: {}\n\n  HINT: Use --agent to specify an agent directly.\n  HINT: Run `ribbon whoami` for a detailed diagnostic.",
                    cwd.display()
                );
            }
        }
    }

    Ok(())
}

// ── pack / unpack ─────────────────────────────────────────────────────

#[derive(Parser)]
struct PackArgs {
    /// Output file (default: events.ndjson.br)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[cfg(feature = "compress")]
fn cmd_pack(args: PackArgs, log_path: &std::path::Path) -> Result<()> {
    let events = read_events(log_path)?;
    let output_path = args
        .output
        .unwrap_or_else(|| log_path.with_extension("ndjson.br"));

    // Serialize all events to a single JSON array
    let json_bytes = serde_json::to_vec(&events)?;

    // Compress with Brotli
    let mut compressed = Vec::new();
    {
        let mut compressor = brotli::CompressorReader::new(
            &json_bytes[..],
            4096, // buffer size
            6,    // compression level (1-11)
            22,   // lgwin
        );
        std::io::copy(&mut compressor, &mut compressed)?;
    }

    std::fs::write(&output_path, &compressed)?;
    let ratio = (compressed.len() as f64 / json_bytes.len() as f64) * 100.0;
    eprintln!(
        "Packed: {} → {} ({:.0}% of original)",
        format_bytes(json_bytes.len()),
        format_bytes(compressed.len()),
        ratio
    );
    Ok(())
}

#[derive(Parser)]
struct UnpackArgs {
    /// Input file (default: events.ndjson.br)
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Output file (default: stdout)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[cfg(feature = "compress")]
fn cmd_unpack(args: UnpackArgs, log_path: &std::path::Path) -> Result<()> {
    let input_path = args
        .input
        .unwrap_or_else(|| log_path.with_extension("ndjson.br"));
    let compressed = std::fs::read(&input_path)?;

    // Decompress with Brotli
    let mut decompressed = Vec::new();
    {
        let mut decompressor = brotli::Decompressor::new(&compressed[..], 4096);
        std::io::copy(&mut decompressor, &mut decompressed)?;
    }

    // Deserialize JSON array
    let events: Vec<RibbonEvent> = serde_json::from_slice(&decompressed)?;

    // Write ndjson
    let ndjson: String = events
        .iter()
        .map(|e| e.to_ndjson_line())
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");

    if let Some(output_path) = args.output {
        std::fs::write(&output_path, ndjson)?;
        eprintln!("Unpacked to {}", output_path.display());
    } else {
        println!("{ndjson}");
    }

    Ok(())
}

// ── pack/unpack dispatchers (feature-gated) ─────────────────────────────

#[cfg(feature = "compress")]
fn pack_handler(args: PackArgs, log_path: &std::path::Path) -> Result<()> {
    cmd_pack(args, log_path)
}

#[cfg(not(feature = "compress"))]
fn pack_handler(_args: PackArgs, _log_path: &std::path::Path) -> Result<()> {
    anyhow::bail!("Pack is not available. Rebuild with: cargo install ribbon --features compress");
}

#[cfg(feature = "compress")]
fn unpack_handler(args: UnpackArgs, log_path: &std::path::Path) -> Result<()> {
    cmd_unpack(args, log_path)
}

#[cfg(not(feature = "compress"))]
fn unpack_handler(_args: UnpackArgs, _log_path: &std::path::Path) -> Result<()> {
    anyhow::bail!(
        "Unpack is not available. Rebuild with: cargo install ribbon --features compress"
    );
}

#[cfg_attr(not(feature = "compress"), allow(dead_code))]
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ── init ──────────────────────────────────────────────────────────────

#[derive(Parser)]
struct InitArgs {
    /// Pre-configure git roots for verification: agent=path
    /// Repeatable. Example: --git-root frontend=. --git-root backend=../api
    #[arg(long = "git-root", value_name = "AGENT=PATH", value_parser = parse_git_root)]
    git_roots: Vec<(String, PathBuf)>,
}

fn parse_git_root(s: &str) -> Result<(String, PathBuf), String> {
    let (agent, path) = s
        .split_once('=')
        .ok_or_else(|| format!("expected AGENT=PATH, got '{s}'. Example: --git-root myagent=."))?;
    Ok((agent.to_string(), PathBuf::from(path)))
}

fn cmd_init(args: InitArgs) -> Result<()> {
    let ribbon_dir = std::path::Path::new(".ribbon");
    if ribbon_dir.exists() {
        anyhow::bail!(".ribbon/ already exists. Remove it first to reinitialize.");
    }

    std::fs::create_dir_all(ribbon_dir)?;

    let mut config = RibbonConfig::default();
    for (agent, path) in &args.git_roots {
        config.git_roots.insert(agent.clone(), path.clone());
        if !config.agents.contains(agent) {
            config.agents.push(agent.clone());
        }
    }

    let toml_str = toml::to_string_pretty(&config)?;
    let config_path = ribbon_dir.join("config.toml");
    std::fs::write(&config_path, toml_str)?;

    eprintln!("Initialized ribbon in .ribbon/");
    eprintln!("  Config: {}", config_path.display());
    eprintln!(
        "  Log:    {} (will be created on first 'ribbon send')",
        config.log_path.display()
    );
    if !args.git_roots.is_empty() {
        eprintln!("  Git roots configured:");
        for (agent, path) in &args.git_roots {
            eprintln!("    {agent} -> {}", path.display());
        }
        eprintln!("  'ribbon verify' will work immediately for these agents.");
    } else {
        eprintln!();
        eprintln!("Next steps:");
        eprintln!("  1. Edit .ribbon/config.toml to add your agents and git roots");
        eprintln!("     (or re-run: ribbon init --git-root myagent=.)");
        eprintln!("  2. ribbon send working --agent my-agent --task 'first task'");
        eprintln!("  3. ribbon status");
    }

    Ok(())
}

// ── main ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load config — searching from project_root or cwd
    let discovered: DiscoveredConfig = match cli.config {
        Some(ref path) => RibbonConfig::from_file(path)?,
        None => RibbonConfig::discover_from(cli.project_root.as_deref())?,
    };
    let config = &discovered.config;

    // Determine log path — resolved relative to config file location
    let log_path = match cli.log {
        Some(ref path) => path.clone(),
        None => discovered.resolve_log_path(),
    };

    match cli.command {
        Commands::Init(args) => cmd_init(args)?,
        Commands::Send(args) => cmd_send(args, &log_path, config)?,
        Commands::Status(args) => cmd_status(args, &log_path)?,
        Commands::Query(args) => cmd_query(args, &log_path)?,
        Commands::Render(args) => cmd_render(args, &log_path)?,
        Commands::Watch(args) => cmd_watch(args, &log_path)?,
        Commands::Verify(args) => cmd_verify(args, &log_path, config)?,
        Commands::Whoami(args) => cmd_whoami(args, config)?,
        Commands::Scope(args) => cmd_scope(args, config)?,
        Commands::Cd(args) => cmd_cd(args, config)?,
        Commands::Pack(args) => pack_handler(args, &log_path)?,
        Commands::Unpack(args) => unpack_handler(args, &log_path)?,
    }

    Ok(())
}
