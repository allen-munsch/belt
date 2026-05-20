//! Ribbon event store — append to ndjson log, read, and query.
//!
//! The ndjson file is the source of truth. All operations are atomic at the
//! line level (each line is a complete JSON object). Appends are O(1) via
//! filesystem append. Reads stream line-by-line for constant memory.

use crate::event::{EventType, RibbonEvent};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Errors that can occur during store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error at line {line}: {source}")]
    Json {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("File not found: {0}")]
    NotFound(PathBuf),
}

/// Append a single event to the ndjson log file.
///
/// Creates the file and parent directories if they don't exist.
/// Each event is written as one line terminated by `\n`.
pub fn append_event(path: &Path, event: &RibbonEvent) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    let line = event
        .to_ndjson_line()
        .map_err(|e| StoreError::Json { line: 0, source: e })?;
    writeln!(file, "{line}")?;
    file.flush()?;
    Ok(())
}

/// Read all events from the ndjson log file.
///
/// Invalid lines are skipped with a warning. Returns events in file order
/// (which is chronological for an append-only log).
pub fn read_events(path: &Path) -> Result<Vec<RibbonEvent>, StoreError> {
    let file = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            StoreError::NotFound(path.to_path_buf())
        } else {
            StoreError::Io(e)
        }
    })?;

    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match RibbonEvent::from_ndjson_line(trimmed) {
            Ok(event) => events.push(event),
            Err(e) => {
                eprintln!(
                    "ribbon: warning: skipping malformed line {}: {}",
                    line_num + 1,
                    e
                );
            }
        }
    }

    Ok(events)
}

/// Query filter for selecting events.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Only events from this agent.
    pub agent: Option<String>,
    /// Only events of this type.
    pub event_type: Option<EventType>,
    /// Only events since this timestamp (inclusive).
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    /// Only events matching this task substring (case-insensitive).
    pub task_contains: Option<String>,
    /// Limit to the last N events.
    pub limit: Option<usize>,
}

impl EventFilter {
    /// Create a filter that matches all events.
    pub fn all() -> Self {
        EventFilter::default()
    }

    /// Filter by agent.
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// Filter by event type.
    pub fn event_type(mut self, et: EventType) -> Self {
        self.event_type = Some(et);
        self
    }

    /// Filter since timestamp.
    pub fn since(mut self, ts: chrono::DateTime<chrono::Utc>) -> Self {
        self.since = Some(ts);
        self
    }

    /// Filter by task substring.
    pub fn task_contains(mut self, task: impl Into<String>) -> Self {
        self.task_contains = Some(task.into());
        self
    }

    /// Limit results.
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    /// Apply this filter to an iterator of events.
    pub fn apply<'a>(&self, events: impl Iterator<Item = &'a RibbonEvent>) -> Vec<&'a RibbonEvent> {
        let mut results: Vec<&RibbonEvent> = events
            .filter(|e| {
                if let Some(ref agent) = self.agent {
                    if &e.agent != agent {
                        return false;
                    }
                }
                if let Some(ref et) = self.event_type {
                    if &e.event_type != et {
                        return false;
                    }
                }
                if let Some(ref since) = self.since {
                    if e.ts < *since {
                        return false;
                    }
                }
                if let Some(ref task_substr) = self.task_contains {
                    if let Some(ref task) = e.task {
                        if !task.to_lowercase().contains(&task_substr.to_lowercase()) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            })
            .collect();

        if let Some(limit) = self.limit {
            let start = results.len().saturating_sub(limit);
            results = results[start..].to_vec();
        }

        results
    }
}

/// Summary of an agent's current state based on its events.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentStatus {
    pub agent: String,
    /// Current state: "idle", "submitted", "working", "committed", "completed",
    /// "failed", "blocked", "confused", "sudo_pending", "hitl_pending".
    pub state: String,
    /// Most recent commit hash if any.
    pub last_commit: Option<String>,
    /// Number of completed tasks total.
    pub completed_count: usize,
    /// Number of failed tasks total.
    pub failed_count: usize,
    /// Whether agent has any active (non-terminal) tasks.
    pub has_active: bool,
    /// The active task description if working.
    pub active_task: Option<String>,
    /// If blocked, what's the blocker (from msg).
    pub blocked_by: Option<String>,
    /// If confused, what's the question (from msg).
    pub confused_about: Option<String>,
    /// If sudo pending, what's requested (from msg).
    pub sudo_request: Option<String>,
    /// If HITL pending, what's needed (from msg).
    pub hitl_request: Option<String>,
    /// Valid next event types the agent can transition to.
    /// E.g. for "submitted": ["working", "failed"]. Empty for terminal states.
    pub next_actions: Vec<String>,
}

/// Compute current status for all agents found in the event log.
pub fn agent_statuses(events: &[RibbonEvent]) -> Vec<AgentStatus> {
    let mut by_agent: HashMap<String, Vec<&RibbonEvent>> = HashMap::new();
    for e in events {
        by_agent.entry(e.agent.clone()).or_default().push(e);
    }

    let mut statuses: Vec<AgentStatus> = by_agent
        .into_iter()
        .map(|(agent, agent_events)| {
            let completed_count = agent_events
                .iter()
                .filter(|e| e.event_type == EventType::Completed)
                .count();
            let failed_count = agent_events
                .iter()
                .filter(|e| e.event_type == EventType::Failed)
                .count();

            let mut state = "idle";
            let mut last_commit = None;
            let mut has_active = false;
            let mut active_task = None;
            let mut blocked_by = None;
            let mut confused_about = None;
            let mut sudo_request = None;
            let mut hitl_request = None;

            // Walk backwards to find current state
            for e in agent_events.iter().rev() {
                match &e.event_type {
                    EventType::Submitted | EventType::Working | EventType::Committed => {
                        if state == "idle" {
                            state = e.event_type.state_name();
                            has_active = true;
                            active_task = e.task.clone();
                        }
                    }
                    EventType::Completed => {
                        if state == "idle" {
                            state = "completed";
                        }
                    }
                    EventType::Failed => {
                        if state == "idle" {
                            state = "failed";
                        }
                    }
                    EventType::Blocked => {
                        if state == "idle" {
                            state = "blocked";
                            has_active = true;
                            active_task = e.task.clone();
                            blocked_by = e.msg.clone();
                        }
                    }
                    EventType::Confused => {
                        if state == "idle" {
                            state = "confused";
                            has_active = true;
                            active_task = e.task.clone();
                            confused_about = e.msg.clone();
                        }
                    }
                    EventType::Sudo => {
                        if state == "idle" {
                            state = "sudo_pending";
                            has_active = true;
                            active_task = e.task.clone();
                            sudo_request = e.msg.clone();
                        }
                    }
                    EventType::Hitl => {
                        if state == "idle" {
                            state = "hitl_pending";
                            has_active = true;
                            active_task = e.task.clone();
                            hitl_request = e.msg.clone();
                        }
                    }
                    // Response events revert to previous state
                    EventType::SudoGranted | EventType::SudoDenied | EventType::HitlResolved => {
                        if state == "idle" {
                            state = "idle";
                        }
                    }
                    EventType::Note => {}
                }
                if last_commit.is_none() {
                    last_commit = e.commit.clone();
                }
                if state != "idle" && last_commit.is_some() {
                    break;
                }
            }

            // Compute valid next actions from this state
            let prev_et = match state {
                "submitted" => Some(EventType::Submitted),
                "working" => Some(EventType::Working),
                "committed" => Some(EventType::Committed),
                "completed" => Some(EventType::Completed),
                "failed" => Some(EventType::Failed),
                "blocked" => Some(EventType::Blocked),
                "confused" => Some(EventType::Confused),
                "sudo_pending" => Some(EventType::Sudo),
                "hitl_pending" => Some(EventType::Hitl),
                _ => None,
            };
            let sm = crate::event::state_machine();
            let next_actions: Vec<String> = sm
                .suggest(prev_et.as_ref())
                .into_iter()
                .map(|(et, _)| et.label().to_lowercase())
                .collect();

            AgentStatus {
                agent,
                state: state.to_string(),
                last_commit,
                completed_count,
                failed_count,
                has_active,
                active_task,
                blocked_by,
                confused_about,
                sudo_request,
                hitl_request,
                next_actions,
            }
        })
        .collect();

    statuses.sort_by(|a, b| a.agent.cmp(&b.agent));
    statuses
}

/// Normalize a task string for fuzzy matching: trim whitespace and lowercase.
fn normalize_task(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Result of finding a previous state — the event type and the canonical task string.
pub type PreviousStateResult = (EventType, Option<String>);

/// Find the previous non-note event for a specific agent+task combination.
///
/// Matching is done in three stages:
/// 1. **Exact match** after trimming (preferred — most precise)
/// 2. **Fuzzy substring match** — either string contains the other (case-insensitive)
/// 3. **Agent-wide fallback** — if the agent has exactly one active task, use that
///
/// Returns (EventType, canonical_task_string) of the last state-changing event,
/// or None if this is a new task. The canonical task string is the one from the
/// matched event — callers should use it for consistency in subsequent events.
pub fn find_previous_state(
    events: &[RibbonEvent],
    agent: &str,
    task: &str,
) -> Option<PreviousStateResult> {
    let query = normalize_task(task);

    // Stage 1: Exact match after trim + lowercase
    for e in events.iter().rev() {
        if e.agent == agent {
            if let Some(ref t) = e.task {
                if normalize_task(t) == query
                    && e.event_type != EventType::Note
                    && !e.event_type.is_response()
                {
                    return Some((e.event_type.clone(), e.task.clone()));
                }
            }
        }
    }

    // Stage 2: Fuzzy substring match (case-insensitive)
    for e in events.iter().rev() {
        if e.agent == agent {
            if let Some(ref t) = e.task {
                let t_norm = normalize_task(t);
                if (t_norm.contains(&query) || query.contains(&t_norm))
                    && e.event_type != EventType::Note
                    && !e.event_type.is_response()
                {
                    eprintln!("  ℹ️  Matched task via fuzzy search: \"{}\"", t.trim());
                    return Some((e.event_type.clone(), e.task.clone()));
                }
            }
        }
    }

    // Stage 3: Agent-wide fallback — exactly one unique active task for this agent
    // First, collect tasks that have been terminated (completed/failed) so we can exclude them.
    let mut terminated_tasks: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for e in events.iter() {
        if e.agent == agent && e.event_type.is_terminal() {
            if let Some(ref t) = e.task {
                terminated_tasks.insert(normalize_task(t));
            }
        }
    }

    let active_states = [
        EventType::Submitted,
        EventType::Working,
        EventType::Committed,
        EventType::Blocked,
        EventType::Confused,
        EventType::Sudo,
        EventType::Hitl,
    ];
    let mut active_task_names: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut latest_event_type: Option<EventType> = None;
    let mut latest_task: Option<String> = None;

    for e in events.iter().rev() {
        if e.agent == agent && active_states.contains(&e.event_type) {
            if let Some(ref t) = e.task {
                let t_norm = normalize_task(t);
                // Skip tasks that have a terminal event later in the log
                if terminated_tasks.contains(&t_norm) {
                    continue;
                }
                active_task_names.insert(t_norm);
                if latest_event_type.is_none() {
                    latest_event_type = Some(e.event_type.clone());
                    latest_task = e.task.clone();
                }
            }
        }
    }

    if active_task_names.len() == 1 {
        if let (Some(et), Some(lt)) = (&latest_event_type, &latest_task) {
            eprintln!("  ℹ️  Matched agent's only active task: \"{}\"", lt.trim());
            return Some((et.clone(), latest_task));
        }
    }

    // No match found — print diagnostic
    if !active_task_names.is_empty() {
        eprintln!(
            "  ⚠️  Could not match task \"{}\" for agent \"{}\".",
            task.trim(),
            agent
        );
        eprintln!("  Active tasks for {}:", agent);
        for name in &active_task_names {
            eprintln!("    - {}", name);
        }
        eprintln!(
            "  HINT: Copy the exact task name from above, or from `ribbon query --agent {}`.",
            agent
        );
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_append_and_read() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path();

        let e1 = RibbonEvent::new("mosaic", EventType::Working).with_task("grpc migration");
        let e2 = RibbonEvent::new("mosaic", EventType::Completed)
            .with_task("grpc migration")
            .with_commit("abc123");

        append_event(path, &e1).unwrap();
        append_event(path, &e2).unwrap();

        let events = read_events(path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, EventType::Working);
        assert_eq!(events[1].event_type, EventType::Completed);
    }

    #[test]
    fn test_filter_by_agent() {
        let events = [
            RibbonEvent::new("alice", EventType::Working),
            RibbonEvent::new("bob", EventType::Completed),
            RibbonEvent::new("alice", EventType::Completed),
        ];

        let filter = EventFilter::all().agent("alice");
        let results = filter.apply(events.iter());
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_filter_limit() {
        let events: Vec<RibbonEvent> = (0..10)
            .map(|i| RibbonEvent::new("agent", EventType::Note).with_msg(format!("msg {i}")))
            .collect();

        let filter = EventFilter::all().limit(3);
        let results = filter.apply(events.iter());
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].msg.as_ref().unwrap(), "msg 7");
    }

    #[test]
    fn test_agent_statuses() {
        let events = vec![
            RibbonEvent::new("mosaic", EventType::Submitted).with_task("task1"),
            RibbonEvent::new("mosaic", EventType::Working).with_task("task1"),
            RibbonEvent::new("mosaic", EventType::Committed)
                .with_task("task1")
                .with_commit("abc"),
            RibbonEvent::new("mosaic", EventType::Completed)
                .with_task("task1")
                .with_commit("abc"),
            RibbonEvent::new("zypi", EventType::Working).with_task("task2"),
        ];

        let statuses = agent_statuses(&events);
        let mosaic = statuses.iter().find(|s| s.agent == "mosaic").unwrap();
        assert_eq!(mosaic.state, "completed");
        assert_eq!(mosaic.completed_count, 1);

        let zypi = statuses.iter().find(|s| s.agent == "zypi").unwrap();
        assert_eq!(zypi.state, "working");
        assert!(zypi.has_active);
    }

    // ── find_previous_state fuzzy matching tests ──────────────────────

    #[test]
    fn test_find_previous_state_exact_match() {
        let events =
            vec![RibbonEvent::new("agent", EventType::Submitted).with_task("Build the gRPC layer")];
        let result = find_previous_state(&events, "agent", "Build the gRPC layer");
        assert!(result.is_some());
        let (et, canonical) = result.unwrap();
        assert_eq!(et, EventType::Submitted);
        assert_eq!(canonical.as_deref(), Some("Build the gRPC layer"));
    }

    #[test]
    fn test_find_previous_state_whitespace_variation() {
        // Extra whitespace in query should still match; returns canonical task
        let events =
            vec![RibbonEvent::new("agent", EventType::Submitted).with_task("Build the gRPC layer")];
        let result = find_previous_state(&events, "agent", "  Build the gRPC layer  ");
        assert!(result.is_some());
        let (et, canonical) = result.unwrap();
        assert_eq!(et, EventType::Submitted);
        assert_eq!(canonical.as_deref(), Some("Build the gRPC layer"));
    }

    #[test]
    fn test_find_previous_state_case_insensitive() {
        let events =
            vec![RibbonEvent::new("agent", EventType::Submitted).with_task("Build the gRPC layer")];
        let result = find_previous_state(&events, "agent", "BUILD THE GRPC LAYER");
        assert!(result.is_some());
        let (et, canonical) = result.unwrap();
        assert_eq!(et, EventType::Submitted);
        assert_eq!(canonical.as_deref(), Some("Build the gRPC layer"));
    }

    #[test]
    fn test_find_previous_state_substring_fallback() {
        // Query is shorter substring of stored task; canonical task returned
        let events = vec![RibbonEvent::new("agent", EventType::Submitted)
            .with_task("IMPLEMENT the PrismFlow formal IR types and type inference engine")];
        let result = find_previous_state(&events, "agent", "PrismFlow formal IR types");
        assert!(result.is_some());
        let (et, canonical) = result.unwrap();
        assert_eq!(et, EventType::Submitted);
        assert_eq!(
            canonical.as_deref(),
            Some("IMPLEMENT the PrismFlow formal IR types and type inference engine")
        );
    }

    #[test]
    fn test_find_previous_state_substring_reverse() {
        // Stored task is shorter, query is longer; canonical task returned
        let events = vec![RibbonEvent::new("agent", EventType::Working).with_task("add dark mode")];
        let result = find_previous_state(&events, "agent", "add dark mode to the whole app");
        assert!(result.is_some());
        let (et, canonical) = result.unwrap();
        assert_eq!(et, EventType::Working);
        assert_eq!(canonical.as_deref(), Some("add dark mode"));
    }

    #[test]
    fn test_find_previous_state_agent_wide_fallback() {
        // No task match at all, but agent has exactly one active task
        let events = vec![RibbonEvent::new("agent", EventType::Submitted).with_task("only task")];
        let result = find_previous_state(&events, "agent", "completely different task name");
        assert!(result.is_some());
        let (et, canonical) = result.unwrap();
        assert_eq!(et, EventType::Submitted);
        assert_eq!(canonical.as_deref(), Some("only task"));
    }

    #[test]
    fn test_find_previous_state_no_match_multiple_tasks() {
        // Agent has multiple active tasks — no fallback
        let events = vec![
            RibbonEvent::new("agent", EventType::Submitted).with_task("task one"),
            RibbonEvent::new("agent", EventType::Submitted).with_task("task two"),
        ];
        let result = find_previous_state(&events, "agent", "something else entirely");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_previous_state_new_task() {
        let events: Vec<RibbonEvent> = vec![];
        let result = find_previous_state(&events, "agent", "new task");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_previous_state_skips_notes() {
        let events = vec![
            RibbonEvent::new("agent", EventType::Submitted).with_task("real work"),
            RibbonEvent::new("agent", EventType::Note)
                .with_task("real work")
                .with_msg("observation"),
        ];
        // Should find Submitted, skipping the Note
        let result = find_previous_state(&events, "agent", "real work");
        assert!(result.is_some());
        let (et, _) = result.unwrap();
        assert_eq!(et, EventType::Submitted);
    }

    #[test]
    fn test_find_previous_state_latest_active_state() {
        // Multiple events for same task — return latest
        let events = vec![
            RibbonEvent::new("agent", EventType::Submitted).with_task("work"),
            RibbonEvent::new("agent", EventType::Working).with_task("work"),
        ];
        let result = find_previous_state(&events, "agent", "work");
        assert!(result.is_some());
        let (et, _) = result.unwrap();
        assert_eq!(et, EventType::Working);
    }

    #[test]
    fn test_find_previous_state_returns_canonical_task() {
        // When fuzzy matched, the canonical task should be from the log
        let events =
            vec![RibbonEvent::new("agent", EventType::Submitted).with_task("Original Task Name")];
        let result = find_previous_state(&events, "agent", "original");
        let (_, canonical) = result.unwrap();
        assert_eq!(canonical.as_deref(), Some("Original Task Name"));
    }
}
