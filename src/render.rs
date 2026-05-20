//! Ribbon render — materialize compact ndjson events into human-readable output.
//!
//! This is the "reification" half of the ribbon system. Agents write compact
//! ndjson (100 bytes/event); `ribbon render` expands it to rich markdown for
//! humans to read. The same events → different views depending on audience.

use crate::event::{EventType, RibbonEvent};

/// Output format for rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderFormat {
    /// Rich markdown with sections, headers, and formatting.
    Markdown,
    /// Plain text, suitable for terminal output or logs.
    Plain,
    /// Compact one-line-per-event format (human-optimized ndjson view).
    Compact,
}

/// Options controlling rendering behavior.
#[derive(Debug, Clone)]
pub struct RenderOpts {
    pub format: RenderFormat,
    /// Only render events since this ISO date (e.g. "2026-05-11").
    pub since: Option<String>,
    /// Only render events for this agent.
    pub agent: Option<String>,
    /// Group events by agent (chronological within each group).
    pub group_by_agent: bool,
    /// Show emoji indicators.
    pub emoji: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts {
            format: RenderFormat::Markdown,
            since: None,
            agent: None,
            group_by_agent: false,
            emoji: true,
        }
    }
}

/// Render a collection of events into a string.
pub fn render(events: &[RibbonEvent], opts: &RenderOpts) -> String {
    // Filter by agent if requested
    let filtered: Vec<&RibbonEvent> = if let Some(ref agent) = opts.agent {
        events.iter().filter(|e| e.agent == *agent).collect()
    } else {
        events.iter().collect()
    };

    if filtered.is_empty() {
        return match opts.format {
            RenderFormat::Markdown => "_No events found._\n".to_string(),
            RenderFormat::Plain => "No events found.\n".to_string(),
            RenderFormat::Compact => String::new(),
        };
    }

    match opts.format {
        RenderFormat::Markdown => render_markdown(&filtered, opts),
        RenderFormat::Plain => render_plain(&filtered, opts),
        RenderFormat::Compact => render_compact(&filtered, opts),
    }
}

fn render_markdown(events: &[&RibbonEvent], opts: &RenderOpts) -> String {
    let mut out = String::new();

    if opts.group_by_agent {
        // Group by agent
        let mut by_agent: std::collections::BTreeMap<&str, Vec<&RibbonEvent>> =
            std::collections::BTreeMap::new();
        for e in events {
            by_agent.entry(&e.agent).or_default().push(e);
        }

        for (agent, agent_events) in &by_agent {
            out.push_str(&format!("## Agent: {agent}\n\n"));
            render_event_list_markdown(&mut out, agent_events, opts);
            out.push('\n');
        }
    } else {
        render_event_list_markdown(&mut out, events, opts);
    }

    out
}

fn render_event_list_markdown(out: &mut String, events: &[&RibbonEvent], opts: &RenderOpts) {
    for event in events {
        let emoji = if opts.emoji {
            event.event_type.emoji()
        } else {
            ""
        };
        let ts = event.ts.format("%Y-%m-%d %H:%M UTC").to_string();

        match event.event_type {
            EventType::Submitted => {
                out.push_str(&format!(
                    "### {emoji} [{ts}] {agent} — SUBMITTED\n",
                    agent = event.agent
                ));
                if let Some(ref task) = event.task {
                    out.push_str(&format!("**Task**: {task}\n"));
                }
                if let Some(ref msg) = event.msg {
                    out.push_str(&format!("{msg}\n"));
                }
            }
            EventType::Working => {
                out.push_str(&format!(
                    "### {emoji} [{ts}] {agent} — WORKING\n",
                    agent = event.agent
                ));
                if let Some(ref task) = event.task {
                    out.push_str(&format!("**Task**: {task}\n"));
                }
                if let Some(ref msg) = event.msg {
                    out.push_str(&format!("{msg}\n"));
                }
            }
            EventType::Committed => {
                out.push_str(&format!(
                    "### {emoji} [{ts}] {agent} — COMMITTED\n",
                    agent = event.agent
                ));
                if let Some(ref commit) = event.commit {
                    out.push_str(&format!("**Commit**: `{commit}`\n"));
                }
                if let Some(ref msg) = event.msg {
                    out.push_str(&format!("{msg}\n"));
                }
            }
            EventType::Completed => {
                out.push_str(&format!(
                    "### {emoji} [{ts}] {agent} — COMPLETED\n",
                    agent = event.agent
                ));
                if let Some(ref task) = event.task {
                    out.push_str(&format!("**Task**: {task}\n"));
                }
                if let Some(ref commit) = event.commit {
                    out.push_str(&format!("**Commit**: `{commit}`\n"));
                }
                if let (Some(tests), Some(failures)) = (event.tests, event.failures) {
                    let status = if failures == 0 { "✅" } else { "⚠️" };
                    out.push_str(&format!(
                        "**Tests**: {tests} passed, {failures} failed {status}\n"
                    ));
                }
                if let Some(ref msg) = event.msg {
                    out.push_str(&format!("{msg}\n"));
                }
            }
            EventType::Failed => {
                out.push_str(&format!(
                    "### {emoji} [{ts}] {agent} — FAILED\n",
                    agent = event.agent
                ));
                if let Some(ref task) = event.task {
                    out.push_str(&format!("**Task**: {task}\n"));
                }
                if let Some(ref msg) = event.msg {
                    out.push_str(&format!("**Error**: {msg}\n"));
                }
            }
            EventType::Note => {
                out.push_str(&format!(
                    "{emoji} [{ts}] **{agent}**: {msg}\n",
                    agent = event.agent,
                    msg = event.msg.as_deref().unwrap_or("")
                ));
            }
            // Coordination states — render as labeled sections
            _ => {
                let label = event.event_type.label();
                out.push_str(&format!(
                    "### {emoji} [{ts}] {agent} — {label}\n",
                    agent = event.agent
                ));
                if let Some(ref task) = event.task {
                    out.push_str(&format!("**Task**: {task}\n"));
                }
                if let Some(ref msg) = event.msg {
                    out.push_str(&format!("{msg}\n"));
                }
                if let Some(ref blocking) = event.blocking {
                    if *blocking {
                        out.push_str("**Blocking**: yes\n");
                    }
                }
            }
        }

        // Extension fields
        if !event.ext.is_empty() {
            for (key, value) in &event.ext {
                out.push_str(&format!("- **{key}**: {value}\n"));
            }
        }

        if !matches!(event.event_type, EventType::Note) {
            out.push('\n');
        }
        out.push('\n');
    }
}

fn render_plain(events: &[&RibbonEvent], _opts: &RenderOpts) -> String {
    let mut out = String::new();

    for event in events {
        let ts = event.ts.format("%Y-%m-%d %H:%M").to_string();
        let label = event.event_type.label();
        let agent = &event.agent;

        match event.event_type {
            EventType::Note => {
                out.push_str(&format!(
                    "[{ts}] {agent}: {msg}\n",
                    msg = event.msg.as_deref().unwrap_or("")
                ));
            }
            _ => {
                out.push_str(&format!("[{ts}] {agent} {label}"));
                if let Some(ref task) = event.task {
                    out.push_str(&format!(" — {task}"));
                }
                if let Some(ref commit) = event.commit {
                    out.push_str(&format!(" ({})", &commit[..commit.len().min(7)]));
                }
                if let Some(ref msg) = event.msg {
                    out.push_str(&format!(" — {msg}"));
                }
                out.push('\n');
            }
        }
    }

    out
}

fn render_compact(events: &[&RibbonEvent], opts: &RenderOpts) -> String {
    let mut out = String::new();

    for event in events {
        let emoji = if opts.emoji {
            event.event_type.emoji()
        } else {
            ""
        };
        let ts = event.ts.format("%m-%d %H:%M").to_string();
        let state_code = match event.event_type {
            EventType::Submitted => "SUB",
            EventType::Working => "WIP",
            EventType::Committed => "CMT",
            EventType::Completed => "DONE",
            EventType::Failed => "FAIL",
            EventType::Note => "NOTE",
            EventType::Blocked => "BLKD",
            EventType::Confused => "CNFS",
            EventType::Sudo => "SUDO",
            EventType::SudoGranted => "SOK",
            EventType::SudoDenied => "SNO",
            EventType::Hitl => "HITL",
            EventType::HitlResolved => "HOK",
        };

        out.push_str(&format!(
            "{emoji} [{ts}] {state_code:<4} {agent:<12}",
            agent = event.agent
        ));

        if let Some(ref task) = event.task {
            out.push_str(&format!(" {task}"));
        }
        if let Some(ref commit) = event.commit {
            out.push_str(&format!(" @{}", &commit[..commit.len().min(7)]));
        }
        if let Some(ref msg) = event.msg {
            if event.event_type == EventType::Note || event.task.is_none() {
                out.push_str(&format!(" {msg}"));
            }
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_markdown() {
        let events = vec![
            RibbonEvent::new("mosaic", EventType::Working).with_task("grpc migration"),
            RibbonEvent::new("mosaic", EventType::Completed)
                .with_task("grpc migration")
                .with_commit("7cba506161d9388cb65394aaad7822eaad2523a3")
                .with_tests(339, 0)
                .with_msg("All passing"),
        ];

        let opts = RenderOpts {
            format: RenderFormat::Markdown,
            emoji: true,
            ..Default::default()
        };

        let result = render(&events, &opts);
        assert!(result.contains("WORKING"));
        assert!(result.contains("COMPLETED"));
        assert!(result.contains("7cba506"));
        assert!(result.contains("339 passed"));
    }

    #[test]
    fn test_render_compact() {
        let events = vec![RibbonEvent::new("zypi", EventType::Completed)
            .with_commit("66faa13")
            .with_msg("gRPC live")];

        let opts = RenderOpts {
            format: RenderFormat::Compact,
            emoji: true,
            ..Default::default()
        };

        let result = render(&events, &opts);
        assert!(result.contains("DONE"));
        assert!(result.contains("zypi"));
        assert!(result.contains("66faa13"));
    }

    #[test]
    fn test_render_empty() {
        let events: Vec<RibbonEvent> = vec![];
        let opts = RenderOpts::default();
        let result = render(&events, &opts);
        assert!(result.contains("No events"));
    }
}
