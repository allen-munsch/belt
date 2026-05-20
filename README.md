# Ribbon — Agent Communication Protocol

**File-first, schema-validated, human+machine readable communication for asynchronous agents.**

Ribbon is a protocol and CLI tool for coordinating multiple agents (human, LLM, or service) through a shared ndjson event log. It's loosely coupled, git-versioned, and bridges to Google's [A2A protocol](https://a2a-protocol.org) for real-time streaming.

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

## Why Ribbon?

| Problem | Ribbon Solution |
|---------|---------------|
| Agents communicate via ad-hoc markdown — fragile `grep` parsing | Structured ndjson, schema-validated |
| Every status check costs hundreds of tokens parsing prose | `ribbon status` returns compact JSON |
| No way to verify agent claims about git commits | `ribbon verify` checks hashes against origin |
| Polling `git fetch` for every check | `ribbon watch` streams events in real time |
| Human-readable output buried in verbose logs | `ribbon render` materializes compact events into rich markdown |
| No standard task lifecycle across teams | Submitted → Working → Committed → Completed/Failed (A2A-compatible) |

## Install

```bash
cargo install ribbon
```

Or build from source:

```bash
git clone https://github.com/allen-munsch/ribbon
cd ribbon
cargo build --release
```

## Quick Start

```bash
# Initialize in your project
ribbon init

# An agent claims a task
ribbon send working \
  --agent frontend-builder \
  --task "add dark mode toggle" \
  --priority HIGH

# Agent commits and completes
ribbon send completed \
  --agent frontend-builder \
  --task "add dark mode toggle" \
  --commit 7cba506161d9388cb65394aaad7822eaad2523a3 \
  --tests 42 --failures 0 \
  --msg "All tests pass, feature ready"

# Check all agents
ribbon status
# AGENT              STATE        COMMIT     DONE   FAIL   ACTIVE TASK
# frontend-builder   ✅ completed 7cba506    3      0      -
# backend-runner     🟡 working  66faa13    5      0      API auth

# Render for humans (markdown)
ribbon render --since 2026-05-11

# Verify git hashes (requires .ribbon/config.toml with git_roots)
ribbon verify

# Watch for new events
ribbon watch
```

## Event Lifecycle

```
submitted → working → committed → completed
                   ↘ failed

note: informational, no state transition
```

Every event is one line of ndjson (newline-delimited JSON). Example:

```jsonl
{"ts":"2026-05-12T03:00:00Z","agent":"frontend-builder","event":"working","task":"dark mode","priority":"HIGH"}
{"ts":"2026-05-12T03:15:00Z","agent":"frontend-builder","event":"committed","commit":"7cba506","msg":"fix contrast ratios"}
{"ts":"2026-05-12T03:16:00Z","agent":"frontend-builder","event":"completed","commit":"7cba506","tests":42,"failures":0}
```

## Commands

| Command | Description |
|---------|-------------|
| `ribbon init` | Create `.ribbon/config.toml` |
| `ribbon send <EVENT> --agent <name>` | Append an event to the log |
| `ribbon status [--json]` | Current state of all agents |
| `ribbon query --agent X --event completed` | Filter events |
| `ribbon render [--format markdown\|plain\|compact]` | Materialize as human-readable output |
| `ribbon watch` | Stream new events (Ctrl+C to stop) |
| `ribbon verify [--agent X]` | Check git hashes against origin |
| `ribbon pack` | Brotli-compress the log for transmission |
| `ribbon unpack` | Decompress a packed log |

## Configuration

Edit `.ribbon/config.toml`:

```toml
# Path to the ndjson event log
log_path = "events.ndjson"

# Known agents (for validation)
agents = ["frontend", "backend", "infra"]

# Git roots for commit verification
[git_roots]
frontend = "packages/frontend"
backend = "services/api"
infra = "."

# Remote and branch for verification
git_remote = "origin"
git_branch = "main"

# Optional A2A bridge endpoint
# a2a_url = "http://localhost:8080/a2a"
```

## Library Usage

Ribbon can be used as a Rust library for embedding in other tools:

```rust
use ribbon::{RibbonEvent, EventType, append_event, RibbonEvent, EventType, append_event, read_events, render, RenderOpts};
use std::path::Path;

// Append an event
let event = RibbonEvent::new("my-agent", EventType::Completed)
    .with_task("finished work")
    .with_commit("abc123");
append_event(Path::new("events.ndjson"), &event)?;

// Read and query
let events = read_events(Path::new("events.ndjson"))?;
let filter = ribbon::EventFilter::all()
    .agent("my-agent")
    .event_type(EventType::Completed);
let completed = filter.apply(events.iter());

// Render for humans
let markdown = render(&events, &RenderOpts::default());
println!("{markdown}");
```

## Reification: Compact → Rich

Ribbon separates the **stored form** (compact ndjson) from the **presented form** (rich markdown). This is the "reification" concept — the same event data, materialized differently for different audiences.

**Stored (100 bytes):**
```json
{"ts":"2026-05-12T04:00:00Z","agent":"builder","event":"completed","task":"dark-mode","commit":"7cba506","tests":42,"failures":0}
```

**Rendered (markdown, `ribbon render`):**
```markdown
### ✅ [2026-05-12 04:00 UTC] builder — COMPLETED
**Task**: dark-mode
**Commit**: `7cba506161d9388cb65394aaad7822eaad2523a3`
**Tests**: 42 passed, 0 failed ✅
```

**Rendered (compact, `ribbon render --format compact`):**
```
✅ [05-12 04:00] DONE builder        dark-mode @7cba506
```

## A2A Bridging

Ribbon's event lifecycle maps directly to [A2A task states](https://a2a-protocol.org):

| Ribbon Event | A2A TaskState |
|------------|---------------|
| `submitted` | `Submitted` |
| `working` | `Working` |
| `completed` | `Completed` + artifacts |
| `failed` | `Failed` + error |

Enable the bridge feature:

```bash
cargo install ribbon --features full
```

Then configure `a2a_url` in `.ribbon/config.toml`.

## Compression

`ribbon pack` compresses the event log with Brotli:

```
Packed: 2.4 KB → 0.7 KB (29% of original)
```

Useful for transmitting event logs between systems or archiving.

## Design Principles

1. **File-first** — No server required. The ndjson file IS the protocol.
2. **Git-native** — Text format, append-only, no merge conflicts.
3. **Schema-validated** — Every event has a well-defined structure.
4. **Dual-mode** — Compact for machines, rendered rich for humans.
5. **A2A-compatible** — Maps to standard agent protocol when you need real-time.
6. **Zero-config start** — `ribbon init` + `ribbon send` works immediately.

## Comparison

| | Ad-hoc .md files | Git Issues | Slack Bots | Ribbon |
|---|---|---|---|---|
| Works offline | ✅ | ❌ | ❌ | ✅ |
| Git-versioned | ✅ | ❌ | ❌ | ✅ |
| Machine-parseable | ❌ (fragile grep) | ❌ (API) | ❌ (API) | ✅ (ndjson) |
| Human-readable | ✅ | ✅ | ✅ | ✅ (via render) |
| Verifiable | ❌ (manual) | ❌ | ❌ | ✅ (git verify) |
| Streamable | ❌ | ❌ | ✅ | ✅ (watch) |
| A2A-compatible | ❌ | ❌ | ❌ | ✅ |


## License

[AGPL-3.0-only](LICENSE)
