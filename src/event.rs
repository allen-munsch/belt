//! Ribbon event types — the canonical communication schema for agent coordination.
//!
//! # Event lifecycle (maps to A2A task states)
//!
//! ```text
//! submitted → working → committed → completed
//!            ↘ blocked  ↕         ↘ failed
//!            ↘ confused ↕
//!            ↘ sudo → sudo_granted / sudo_denied
//!            ↘ hitl  → hitl_resolved
//!
//! note: informational, no state transition
//! ```
//!
//! # State Machine
//!
//! Ribbon enforces a hardcoded state machine. Each event type has valid
//! predecessors and successors. Invalid transitions are rejected with
//! breadcrumb hints suggesting valid next steps.
//!
//! Every event is a single line of ndjson (newline-delimited JSON).
//! The file is append-only, git-versioned, and streamable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The canonical event type for agent communication.
///
/// Designed to be compact yet human-readable in raw form.
/// When compressed (ribbon pack), the ndjson typically shrinks 80%+ with Brotli.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RibbonEvent {
    /// ISO 8601 timestamp with timezone. Always set on creation.
    pub ts: DateTime<Utc>,

    /// Agent identifier — e.g. "frontend", "backend", "human:alice".
    /// Free-form but should be consistent within a project.
    pub agent: String,

    /// Event type — determines the state transition (or lack thereof).
    #[serde(rename = "event")]
    pub event_type: EventType,

    // ── Optional standard fields ──
    /// Task identifier or human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,

    /// Git commit hash (full SHA, not short). Required for `committed` and `completed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,

    /// Human-readable message. Free-form but concise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,

    /// Test count (for completed events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<u32>,

    /// Test failure count (0 = all passing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failures: Option<u32>,

    /// Priority: "LOW", "MEDIUM", "HIGH", "CRITICAL".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,

    /// Whether this blocks downstream work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,

    /// Ribbon crate version that created this event.
    /// Set automatically. Enables forward migration when the event schema changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Extension fields — arbitrary key-value pairs for project-specific data.
    /// Example: `{"branch": "feat/grpc", "pr": "https://..."}`
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ext: HashMap<String, serde_json::Value>,
}

/// Event type — the state transition or informational marker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    /// Task has been requested but not yet claimed.
    Submitted,
    /// Agent has claimed the task and started working.
    Working,
    /// Code has been committed locally but not yet verified/pushed.
    /// Ribbon convention: committed → ribbon verify → completed.
    Committed,
    /// Task is complete — committed, verified, pushed, tested.
    Completed,
    /// Task has failed. Must include an error message.
    Failed,
    /// Informational note. No state transition. Use for observations, decisions, etc.
    Note,

    // ── Coordination states ──
    /// Agent is blocked waiting on a dependency (another agent, external service, etc).
    /// Requires `msg` explaining what's blocking and who can unblock.
    Blocked,
    /// Agent is confused and needs clarification from a human or peer.
    /// Requires `msg` with the question.
    Confused,
    /// Agent requests elevated permission (e.g., to cross scope boundaries).
    /// Requires `msg` explaining what sudo is needed for.
    Sudo,
    /// Elevated permission granted. Transitions back to previous state.
    #[serde(rename = "sudo_granted")]
    SudoGranted,
    /// Elevated permission denied. Transitions back to previous state with explanation.
    #[serde(rename = "sudo_denied")]
    SudoDenied,
    /// Human-in-the-loop intervention requested.
    /// Requires `msg` describing what the human needs to do.
    Hitl,
    /// Human resolved the HITL request. Transitions back to previous state.
    #[serde(rename = "hitl_resolved")]
    HitlResolved,
}

impl EventType {
    /// Returns true if this event type represents a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, EventType::Completed | EventType::Failed)
    }

    /// Returns true if this event type represents an active/in-flight state
    /// where the agent is doing work.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            EventType::Submitted | EventType::Working | EventType::Committed
        )
    }

    /// Returns true if this is a coordination state (blocked, confused, sudo, hitl).
    pub fn is_coordination(&self) -> bool {
        matches!(
            self,
            EventType::Blocked
                | EventType::Confused
                | EventType::Sudo
                | EventType::SudoGranted
                | EventType::SudoDenied
                | EventType::Hitl
                | EventType::HitlResolved
        )
    }

    /// Returns true if this event is a response (granted/denied/resolved).
    pub fn is_response(&self) -> bool {
        matches!(
            self,
            EventType::SudoGranted | EventType::SudoDenied | EventType::HitlResolved
        )
    }

    /// Returns the state this event would put the agent in (for status tracking).
    /// Returns None for note (no state change) and responses (revert to previous).
    pub fn state_name(&self) -> &'static str {
        match self {
            EventType::Submitted => "submitted",
            EventType::Working => "working",
            EventType::Committed => "committed",
            EventType::Completed => "completed",
            EventType::Failed => "failed",
            EventType::Note => "idle",
            EventType::Blocked => "blocked",
            EventType::Confused => "confused",
            EventType::Sudo => "sudo_pending",
            EventType::SudoGranted => "idle",      // reverts to previous
            EventType::SudoDenied => "idle",        // reverts to previous
            EventType::Hitl => "hitl_pending",
            EventType::HitlResolved => "idle",      // reverts to previous
        }
    }

    /// Human-readable label for rendering.
    pub fn label(&self) -> &'static str {
        match self {
            EventType::Submitted => "SUBMITTED",
            EventType::Working => "WORKING",
            EventType::Committed => "COMMITTED",
            EventType::Completed => "COMPLETED",
            EventType::Failed => "FAILED",
            EventType::Note => "NOTE",
            EventType::Blocked => "BLOCKED",
            EventType::Confused => "CONFUSED",
            EventType::Sudo => "SUDO",
            EventType::SudoGranted => "SUDO_GRANTED",
            EventType::SudoDenied => "SUDO_DENIED",
            EventType::Hitl => "HITL",
            EventType::HitlResolved => "HITL_RESOLVED",
        }
    }

    /// Simple emoji for compact rendering.
    pub fn emoji(&self) -> &'static str {
        match self {
            EventType::Submitted => "📥",
            EventType::Working => "🟡",
            EventType::Committed => "📦",
            EventType::Completed => "✅",
            EventType::Failed => "❌",
            EventType::Note => "📝",
            EventType::Blocked => "🚫",
            EventType::Confused => "❓",
            EventType::Sudo => "🔐",
            EventType::SudoGranted => "🔓",
            EventType::SudoDenied => "🔒",
            EventType::Hitl => "👤",
            EventType::HitlResolved => "👍",
        }
    }
}

// ── State Machine ─────────────────────────────────────────────────────

/// One valid transition in the state machine.
#[derive(Debug, Clone)]
pub struct Transition {
    /// The event being sent (target state).
    pub event: EventType,
    /// Required fields for this transition.
    pub required_fields: &'static [&'static str],
    /// Breadcrumb — what the agent should do next after this transition.
    pub breadcrumb: &'static str,
}

/// The ribbon state machine — hardcoded valid transitions with breadcrumb hints.
///
/// Design principles:
/// 1. Every non-note event must have a valid predecessor state.
/// 2. Invalid transitions are rejected with breadcrumb hints.
/// 3. Responses (sudo_granted, sudo_denied, hitl_resolved) require a pending request.
/// 4. Use `--force` to bypass validation in emergencies.
#[derive(Debug, Clone, Default)]
pub struct StateMachine {
    /// Map: current state → list of valid next transitions.
    /// `None` key means "no previous state" (fresh task).
    transitions: HashMap<Option<EventType>, Vec<Transition>>,
}

impl StateMachine {
    /// Build the hardcoded state machine.
    pub fn new() -> Self {
        use EventType::*;
        let mut sm = StateMachine::default();

        // ── From nothing (task creation) ──
        sm.add(
            None,
            Submitted,
            &["task"],
            "Next: use `ribbon send working --task \"...\"` to claim this task.",
        );

        // ── From submitted ──
        sm.add(
            Some(Submitted),
            Submitted,
            &[],
            "Task amended. The new task text replaces the previous one. Next: `ribbon send working --task \"...\"` to claim this task.",
        );
        sm.add(
            Some(Submitted),
            Working,
            &[],
            "Next: work on the task, then `ribbon send committed --commit <SHA>` when code is ready.",
        );
        sm.add(
            Some(Submitted),
            Failed,
            &["msg"],
            "Make sure to include --msg explaining why this task cannot proceed.",
        );

        // ── From working ──
        sm.add(
            Some(Working),
            Committed,
            &["commit"],
            "Next: run `ribbon verify` to check your commit exists on origin, then `ribbon send completed --tests N --failures 0`.",
        );
        sm.add(
            Some(Working),
            Failed,
            &["msg"],
            "Include --msg explaining what went wrong. Consider `ribbon send blocked` or `ribbon send confused` first if the problem might be resolvable.",
        );
        sm.add(
            Some(Working),
            Blocked,
            &["msg"],
            "Describe what you're blocked on and who can unblock you. Use `ribbon send working` when the blocker is resolved.",
        );
        sm.add(
            Some(Working),
            Confused,
            &["msg"],
            "Ask your question clearly. A human or peer agent will respond. Use `ribbon send working` when you have clarity.",
        );
        sm.add(
            Some(Working),
            Sudo,
            &["msg"],
            "Explain what elevated permission you need and why. Wait for `sudo_granted` or `sudo_denied` before proceeding.",
        );
        sm.add(
            Some(Working),
            Hitl,
            &["msg"],
            "Describe what the human needs to do. The task will stay in HITL state until `hitl_resolved` is sent.",
        );

        // ── From committed ──
        sm.add(
            Some(Committed),
            Completed,
            &[],
            "Done! Run `ribbon status` to confirm, or `ribbon render` to see your project's timeline.",
        );
        sm.add(
            Some(Committed),
            Failed,
            &["msg"],
            "Tests failing or verification issue? Include --msg with details.",
        );
        sm.add(
            Some(Committed),
            Working,
            &[],
            "Going back to work — maybe more changes needed. Next: `ribbon send committed --commit <SHA>` when ready.",
        );

        // ── From blocked ──
        sm.add(
            Some(Blocked),
            Working,
            &[],
            "Blocker resolved! Continue working.",
        );
        sm.add(
            Some(Blocked),
            Failed,
            &["msg"],
            "Blocker cannot be resolved. Include --msg explaining why.",
        );
        sm.add(
            Some(Blocked),
            Confused,
            &["msg"],
            "Not sure how to resolve the blocker? Ask for help.",
        );

        // ── From confused ──
        sm.add(
            Some(Confused),
            Working,
            &[],
            "Got clarity! Back to work.",
        );
        sm.add(
            Some(Confused),
            Failed,
            &["msg"],
            "Cannot resolve confusion. Include --msg with what's unclear.",
        );

        sm
    }

    fn add(
        &mut self,
        from: Option<EventType>,
        event: EventType,
        required_fields: &'static [&'static str],
        breadcrumb: &'static str,
    ) {
        self.transitions
            .entry(from)
            .or_default()
            .push(Transition {
                event,
                required_fields,
                breadcrumb,
            });
    }

    /// Validate a proposed transition.
    /// Returns Ok with breadcrumb hint on success, Err with explanation on failure.
    pub fn validate(
        &self,
        prev_state: Option<&EventType>,
        event: &EventType,
    ) -> Result<&'static str, String> {
        // Note events are always allowed
        if event == &EventType::Note {
            return Ok("Note recorded. No state change.");
        }

        // Response events have special validation
        if event.is_response() {
            return self.validate_response(prev_state, event);
        }

        // From terminal states, only submitted (new task) is allowed
        if let Some(ps) = prev_state {
            if ps.is_terminal() {
                let task_hint = if ps == &EventType::Completed {
                    "This task is already completed. "
                } else {
                    "This task has already failed. "
                };
                return Err(format!(
                    "{}You are in '{}' state (terminal).\n\n  To start a new task: ribbon send submitted --task \"new task description\" --agent <agent>\n  To see all tasks: ribbon status\n  To review history: ribbon query --agent <agent>",
                    task_hint, ps.state_name()
                ));
            }
        }

        // Look up valid transitions
        let key = prev_state.cloned();
        if let Some(valid) = self.transitions.get(&key) {
            if let Some(transition) = valid.iter().find(|t| t.event == *event) {
                return Ok(transition.breadcrumb);
            }
        }

        // Invalid transition — build helpful error
        let from_name = prev_state
            .map(|s| s.state_name())
            .unwrap_or("(new task)");
        let to_name = event.state_name();

        let mut hints = format!(
            "You are in '{}' state, but tried to go to '{}'.\n\n  This is not allowed.\n",
            from_name, to_name
        );

        // Show valid next steps
        if let Some(valid) = self.transitions.get(&key) {
            if valid.is_empty() {
                hints.push_str("  This state has no further transitions.\n");
                hints.push_str("  Create a new task: ribbon send submitted --task \"...\" --agent <agent>\n");
            } else {
                hints.push_str("  What you CAN do from here:\n");
                for t in valid {
                    let label = t.event.label().to_lowercase();
                    hints.push_str(&format!("    ribbon send {} --agent <agent> --task \"...\"", label));
                    if !t.required_fields.is_empty() {
                        hints.push_str(&format!(
                            " {}",
                            t.required_fields
                                .iter()
                                .map(|f| format!("--{f} ..."))
                                .collect::<Vec<_>>()
                                .join(" ")
                        ));
                    }
                    hints.push('\n');
                }
            }
        } else {
            hints.push_str("  To see current state: ribbon status\n");
            hints.push_str("  To review history: ribbon query --agent <agent>\n");
        }

        hints.push_str("\n  Run `ribbon status` to see your current state and active tasks.\n");

        Err(hints)
    }

    /// Validate response events (sudo_granted, sudo_denied, hitl_resolved).
    fn validate_response(
        &self,
        prev_state: Option<&EventType>,
        event: &EventType,
    ) -> Result<&'static str, String> {
        match event {
            EventType::SudoGranted => {
                if prev_state == Some(&EventType::Sudo) {
                    Ok("Sudo granted! You may now proceed with elevated permissions. Next: `ribbon send working` to resume.")
                } else {
                    Err(format!(
                        "`sudo_granted` requires a pending `sudo` as the previous state.\n\n  Current: {}\n  HINT: Only respond to a sudo request with `sudo_granted` or `sudo_denied`.",
                        prev_state.map(|s| s.state_name()).unwrap_or("(none)")
                    ))
                }
            }
            EventType::SudoDenied => {
                if prev_state == Some(&EventType::Sudo) {
                    Ok("Sudo denied. Consider `ribbon send failed` if this blocks completion, or `ribbon send working` for an alternative approach.")
                } else {
                    Err(format!(
                        "`sudo_denied` requires a pending `sudo` as the previous state.\n\n  Current: {}\n  HINT: Only respond to a sudo request with `sudo_granted` or `sudo_denied`.",
                        prev_state.map(|s| s.state_name()).unwrap_or("(none)")
                    ))
                }
            }
            EventType::HitlResolved => {
                if prev_state == Some(&EventType::Hitl) {
                    Ok("HITL resolved! Next: `ribbon send working` to resume.")
                } else {
                    Err(format!(
                        "`hitl_resolved` requires a pending `hitl` as the previous state.\n\n  Current: {}\n  HINT: Only resolve an active HITL request with `hitl_resolved`.",
                        prev_state.map(|s| s.state_name()).unwrap_or("(none)")
                    ))
                }
            }
            _ => unreachable!(),
        }
    }

    /// Suggest the valid next event types from a given state.
    pub fn suggest(&self, prev_state: Option<&EventType>) -> Vec<(EventType, &'static str)> {
        if let Some(ps) = prev_state {
            if ps.is_terminal() {
                return vec![(EventType::Submitted, "Create a new task to start fresh.")];
            }
        }
        match self.transitions.get(&prev_state.cloned()) {
            Some(valid) => valid.iter().map(|t| (t.event.clone(), t.breadcrumb)).collect(),
            None => vec![],
        }
    }
}

/// Global state machine instance (lazily constructed singleton).
pub fn state_machine() -> StateMachine {
    StateMachine::new()
}

/// Current ribbon crate version — embedded in every event for forward compatibility.
pub const RIBBON_VERSION: &str = env!("CARGO_PKG_VERSION");

impl RibbonEvent {
    /// Create a new event with current timestamp and version.
    pub fn new(agent: impl Into<String>, event_type: EventType) -> Self {
        RibbonEvent {
            ts: Utc::now(),
            agent: agent.into(),
            event_type,
            task: None,
            commit: None,
            msg: None,
            tests: None,
            failures: None,
            priority: None,
            blocking: None,
            version: Some(RIBBON_VERSION.to_string()),
            ext: HashMap::new(),
        }
    }

    /// Builder: set task description.
    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    /// Builder: set commit hash.
    pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
        self.commit = Some(commit.into());
        self
    }

    /// Builder: set message.
    pub fn with_msg(mut self, msg: impl Into<String>) -> Self {
        self.msg = Some(msg.into());
        self
    }

    /// Builder: set test results.
    pub fn with_tests(mut self, tests: u32, failures: u32) -> Self {
        self.tests = Some(tests);
        self.failures = Some(failures);
        self
    }

    /// Builder: set priority.
    pub fn with_priority(mut self, priority: impl Into<String>) -> Self {
        self.priority = Some(priority.into());
        self
    }

    /// Builder: set blocking flag.
    pub fn with_blocking(mut self, blocking: bool) -> Self {
        self.blocking = Some(blocking);
        self
    }

    /// Builder: add an extension field.
    pub fn with_ext(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.ext.insert(key.into(), value.into());
        self
    }

    /// Serialize to a single ndjson line (no trailing newline — caller adds if needed).
    pub fn to_ndjson_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from a single ndjson line.
    pub fn from_ndjson_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_roundtrip() {
        let event = RibbonEvent::new("mosaic", EventType::Completed)
            .with_task("gRPC migration")
            .with_commit("7cba506161d9388cb65394aaad7822eaad2523a3")
            .with_msg("All tests passing, gRPC live on :4041")
            .with_tests(339, 0)
            .with_priority("HIGH")
            .with_blocking(false)
            .with_ext("branch", "feat/grpc");

        let line = event.to_ndjson_line().unwrap();
        let parsed = RibbonEvent::from_ndjson_line(&line).unwrap();

        assert_eq!(parsed.agent, "mosaic");
        assert_eq!(parsed.event_type, EventType::Completed);
        assert_eq!(
            parsed.commit.unwrap(),
            "7cba506161d9388cb65394aaad7822eaad2523a3"
        );
        assert_eq!(parsed.tests, Some(339));
        assert_eq!(parsed.failures, Some(0));
        assert_eq!(parsed.ext.get("branch").unwrap(), "feat/grpc");
    }

    #[test]
    fn test_terminal_states() {
        assert!(EventType::Completed.is_terminal());
        assert!(EventType::Failed.is_terminal());
        assert!(!EventType::Working.is_terminal());
        assert!(!EventType::Note.is_terminal());
    }

    #[test]
    fn test_minimal_event() {
        let event =
            RibbonEvent::new("human:alice", EventType::Note).with_msg("Thinking about architecture");
        let line = event.to_ndjson_line().unwrap();
        assert!(line.contains("human:alice"));
        assert!(line.contains("note"));
    }
}
