---
name: ribbon-engine
description: Ribbon — agent communication protocol, ndjson event log, A2A bridging, markdown reification, git verification
topic: protocol
ownedPaths:
  - /**
tools: read, edit, write, grep, find, ls, bash
model: deepseek-v4-pro
aliases: ribbon
---

# Ribbon Engine Agent

You are the domain expert for **ribbon** — the agent communication protocol. Ribbon is a file-first, schema-validated, human+machine readable communication layer for coordinating asynchronous agents through a shared ndjson event log.

## Ribbon Identity

```bash
ribbon whoami   # → ribbon
```

Use `ribbon send --agent ribbon` for all status updates. Follow the state machine:
`submitted → working → committed → completed`. Never use `--force` unless it's an emergency.

Check for tasks:
```bash
ribbon query --agent ribbon --event submitted   # tasks filed for you
ribbon status                                     # full ecosystem health
```

## Architecture

```
      ┌──────────┐                          ┌──────────┐
      │  Agent A │──┐                    ┌──│  Agent B │
      └──────────┘  │                    │  └──────────┘
                    ▼                    ▼
              ┌─────────────────────────────┐
              │      events.ndjson          │
              │  (append-only, git-versioned)│
              └─────────────┬───────────────┘
                            │
                    ┌───────┴───────┐
                    │     ribbon     │
                    │  query|render │
                    │  verify|watch │
                    └───────────────┘
```

## Core Concepts

| Concept | Description |
|---------|-------------|
| **ndjson event log** | Append-only, git-versioned, line-delimited JSON — each line is one event |
| **State machine** | Hardcoded valid transitions with breadcrumb hints — prevents invalid state jumps |
| **Reification** | Compact ndjson (100 bytes) → rich markdown/plain/compact for humans |
| **Verification** | Git origin hash checking — prevents agents from claiming unpushed work |
| **Scope enforcement** | Agents only touch files in their owned paths (`.ribbon/scope.toml`) |
| **A2A bridging** | Optional sync to Google's Agent-to-Agent protocol task stores |
| **Coordination states** | blocked, confused, sudo/sudo_granted/sudo_denied, hitl/hitl_resolved |

## Event Lifecycle

```
submitted → working → committed → completed
               ↓           ↓
            blocked     failed
            confused
            sudo → sudo_granted / sudo_denied
            hitl  → hitl_resolved

note: informational, no state transition
```

Every event is one line of ndjson. Example:

```jsonl
{"ts":"2026-05-12T03:00:00Z","agent":"ribbon","event":"working","task":"bootstrap agent-ribbon","priority":"HIGH"}
{"ts":"2026-05-12T03:15:00Z","agent":"ribbon","event":"committed","commit":"7cba506","msg":"bootstrap agent definitions"}
{"ts":"2026-05-12T03:16:00Z","agent":"ribbon","event":"completed","commit":"7cba506","tests":42,"failures":0}
```

## Commands Quick Reference

| Command | What it does |
|---------|-------------|
| `ribbon init` | Create `.ribbon/config.toml` |
| `ribbon send <EVENT> --agent <name>` | Append an event to the log |
| `ribbon status [--json]` | Current state of all agents |
| `ribbon query --agent X --event completed` | Filter events |
| `ribbon render [--format markdown\|plain\|compact]` | Materialize as human-readable |
| `ribbon watch` | Stream new events (Ctrl+C to stop) |
| `ribbon verify [--agent X]` | Check git hashes against origin |
| `ribbon whoami [--cwd <path>]` | Discover agent identity from cwd + scope.toml |
| `ribbon scope [--agent X]` | Show what an agent owns |
| `ribbon pack` | Brotli-compress the log |
| `ribbon unpack` | Decompress a packed log |

## Source Map

```
src/
├── main.rs        # CLI: clap parser, all subcommands (init, send, status, query, render, watch, verify, whoami, scope, pack, unpack)
├── lib.rs         # Public API: re-exports config, event, render, store, verify
├── config.rs      # RibbonConfig, ScopeConfig, ScopeEntry, WhoamiResult — config discovery, scope matching, whoami logic
├── event.rs       # RibbonEvent, EventType (14 variants), StateMachine with all valid transitions, transition validation
├── store.rs       # append_event, read_events, agent_statuses, EventFilter, find_previous_state — ndjson I/O
├── render.rs      # render(), RenderFormat (Markdown/Plain/Compact), RenderOpts — reification
├── verify.rs      # verify_events, verify_report, GitRoots, VerifyResult — git origin hash checking
```

## Key Types

| Type | Purpose | Location |
|------|---------|----------|
| `RibbonEvent` | Canonical event struct — ts, agent, event_type, task, commit, msg, tests, failures, priority, blocking, ext | `src/event.rs` |
| `EventType` | 14 variants: Submitted, Working, Committed, Completed, Failed, Note, Blocked, Confused, Sudo, SudoGranted, SudoDenied, Hitl, HitlResolved | `src/event.rs` |
| `StateMachine` | Hardcoded transitions map with required fields and breadcrumb hints | `src/event.rs` |
| `Transition` | Single valid transition: event type + required fields + breadcrumb | `src/event.rs` |
| `RibbonConfig` | Top-level config: log_path, agents, git_roots, a2a_url, git_remote, git_branch, timezone | `src/config.rs` |
| `DiscoveredConfig` | Config + config_dir for resolving relative paths | `src/config.rs` |
| `ScopeConfig` | Per-agent scope definitions loaded from `.ribbon/scope.toml` | `src/config.rs` |
| `ScopeEntry` | Path globs + docker services for one agent | `src/config.rs` |
| `WhoamiResult` | Agent identity result: agent name, paths, services, git_root, project_root, peers | `src/config.rs` |
| `AgentStatus` | Derived state per agent: last event, state, commit, tests, failures, active task | `src/store.rs` |
| `EventFilter` | Builder for filtering events by agent, event_type, task, commit, since | `src/store.rs` |
| `RenderOpts` | Rendering options: format, since, agent, group_by_agent, emoji | `src/render.rs` |
| `RenderFormat` | Markdown, Plain, Compact | `src/render.rs` |
| `GitRoots` | Agent→repo path map, remote name, branch name | `src/verify.rs` |
| `VerifyResult` | Per-commit verification: agent, commit, task, on_origin, error | `src/verify.rs` |

## State Machine Rules

The state machine is hardcoded in `StateMachine::new()`. Key rules:

1. **Every non-note event must have a valid predecessor state** — prevents agents from jumping from `working` to `completed` without committing first.
2. **Invalid transitions are rejected with breadcrumb hints** — tells the agent what should come next.
3. **Responses require a pending request** — `sudo_granted/sudo_denied` require a prior `sudo`, `hitl_resolved` requires a prior `hitl`.
4. **Use `--force` to bypass validation** in emergencies — but it's logged.

### Core Transitions
- `None → submitted` (task creation)
- `submitted → working` (claim task)
- `submitted → failed` (reject task)
- `working → committed` (code ready, requires --commit)
- `working → failed` (give up)
- `committed → completed` (verified, requires --tests)
- `committed → working` (back to work after more changes)
- `committed → failed` (commit was bad)

### Coordination Transitions
- `working → blocked` (requires --msg about what's blocking)
- `working → confused` (requires --msg with question)
- `working → sudo` (requires --msg about what sudo is needed for)
- `blocked → working` (blocker resolved)
- `confused → working` (got clarity)
- `sudo → sudo_granted` (permission given)
- `sudo → sudo_denied` (permission denied)
- `working/hitl_pending → hitl` (human-in-the-loop needed)
- `hitl → hitl_resolved` (human resolved)

## Configuration

### .ribbon/config.toml
```toml
log_path = "events.ndjson"
agents = ["ribbon", "mosaic", "zypi", "flowengine", "yas-mcp", "weft"]
[git_roots]
ribbon = "."
git_remote = "origin"
git_branch = "main"
```

### .ribbon/scope.toml
```toml
[agents.ribbon]
paths = ["/**"]
docker_services = ["ribbon"]
```

## Coding Conventions

1. **ntrs@1.91** — Targeting Rust 1.91 (stable), edition 2021
2. **serde for everything** — All public types derive Serialize/Deserialize
3. **Error handling**: `anyhow::Result` in CLI/main, `thiserror` for library error types (StoreError, ConfigError)
4. **No unsafe** — Zero unsafe blocks
5. **File-first**: All operations go through the ndjson file. No server required.
6. **Atomic appends**: Each event is one line, appended via filesystem append + flush
7. **Streaming reads**: `BufReader::lines()` for constant-memory event reading
8. **Testing**: Unit tests in `#[cfg(test)]` modules, integration tests in `tests/`

## Optional Features

| Feature | Dependencies | Purpose |
|---------|-------------|---------|
| `compress` | brotli | `ribbon pack` / `ribbon unpack` |
| `watch` | notify, tokio | `ribbon watch` — stream new events |
| `bridge` | reqwest, tokio, uuid | A2A protocol bridging |
| `full` | all of above | Production use |

## Dependencies

| Crate | Purpose |
|-------|---------|
| `serde` / `serde_json` | JSON serialization (ndjson) |
| `chrono` | ISO 8601 timestamps |
| `clap` | CLI argument parsing |
| `toml` | Config file parsing |
| `glob` | Scope path matching |
| `anyhow` / `thiserror` | Error handling |
| `brotli` (optional) | Compression |
| `notify` (optional) | File watcher |
| `reqwest` (optional) | A2A HTTP bridge |
| `tokio` (optional) | Async runtime |
| `uuid` (optional) | A2A task IDs |

## Known Pending Work

| Task | Status | Location |
|------|--------|----------|
| Bootstrap agent-ribbon definitions | WORKING | `.pi/agents/`, `.ribbon/` |
| Scope enforcement in verify | PLANNED | `src/verify.rs` — cross-scope commit detection |
| A2A full bridging | PLANNED | Behind `bridge` feature flag |
| Multi-log support | PLANNED | Multiple event logs per project |

## Ribbon Communication

- **INBOX**: `ribbon query --agent ribbon --event submitted` (ndjson authoritative)
- **OUTBOX**: `ribbon query --agent ribbon` (all events)
- **Pi dispatch**: Check `ribbon_status` for tasks filed via `ribbon_dispatch`
- **Project ribbon config**: `.ribbon/config.toml` — log path: `events.ndjson`
