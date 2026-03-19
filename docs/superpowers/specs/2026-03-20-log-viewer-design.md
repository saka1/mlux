# Log Viewer Mode — Design Spec

## Problem

Debugging the mlux viewer currently requires `--log /path` and a separate terminal
with `tail -f`. This is friction: window switching, correlating log timing with
viewer actions, and extracting log snippets for issues are all cumbersome.

## Solution

Add an in-TUI log viewer mode backed by an always-on ring buffer logger.

## Architecture

### Custom Logger (`src/log.rs`)

- `RingLog` struct implementing `log::Log`
- Internal storage: `Arc<Mutex<VecDeque<LogEntry>>>` with fixed capacity (1024 entries)
- `LogEntry`: `{ timestamp: Instant, level: Level, target: String, message: String }`
- On `log()`: push to ring buffer (drop oldest on overflow); optionally write to file
- `env_filter` crate for `RUST_LOG` parsing (same crate `env_logger` uses internally)

### Log Level Control

| Condition              | Ring buffer  | File output  |
|------------------------|--------------|--------------|
| Default                | info+        | none         |
| `--debug`              | debug+       | none         |
| `--log path`           | info+        | info+        |
| `--debug --log path`   | debug+       | debug+       |

`RUST_LOG` env var overrides the above when set.

### Logger Initialization (`main.rs`)

- Always initialize `RingLog` (currently viewer mode skips logger init)
- `env_logger` dependency removed; replaced by `RingLog` + `env_filter`
- `--debug` CLI flag added
- `Arc` clone of ring buffer passed to viewer

### Log Viewer Mode (`src/viewer/mode_log.rs`)

New `ViewerMode::Log(LogState)` variant.

**LogState:**
- `scroll_offset: usize`
- `search_query: Option<String>` — active `/` search
- `search_matches: Vec<usize>` — matching line indices
- `search_index: usize` — current match position

**Display:**
- Each line: `[HH:MM:SS.mmm] [LEVEL] target: message`
- Color by level: ERROR=red, WARN=yellow, INFO=default, DEBUG=dim
- Search matches highlighted

**Key bindings:**
- `j/k`, `Up/Down` — scroll
- `g/G` — top/bottom
- `/` — enter search (incremental, regex)
- `n/N` — next/previous match
- `y` — yank all log text to clipboard (`Effect::Yank`)
- `q/Esc` — return to Normal mode

**Entry point:**
- `:log` command in `mode_command.rs` → `Effect::SetMode(ViewerMode::Log(...))`

### Integration Points

| File                        | Change                                              |
|-----------------------------|-----------------------------------------------------|
| `src/log.rs` (new)          | `RingLog`, `LogEntry`, ring buffer, `Log` impl       |
| `src/main.rs`               | Always init logger, `--debug` flag                   |
| `src/viewer/effect.rs`      | `ViewerMode::Log(LogState)`, buffer ref in ViewContext|
| `src/viewer/mode_log.rs` (new) | Mode handler, display, scroll, search, yank        |
| `src/viewer/mode_command.rs`| `:log` command                                       |
| `src/viewer/mod.rs`         | Mode routing for Log                                 |
| `src/viewer/keymap.rs`      | Log mode key mappings                                |

## Design Decisions

- **`Mutex` over lock-free:** Log writes are sub-microsecond push operations with
  near-zero contention. Lock-free ring buffers (SPSC only, or no iteration support)
  add complexity without measurable benefit.
- **Always-on buffer:** Info-level logging is low volume and has no I/O.
  The ring buffer overhead (1024 entries, no file writes) is negligible compared
  to font data and binary size.
- **Replaces `env_logger`:** Rather than a multi-logger wrapper, a single custom
  logger that handles both buffer and file output is simpler.
- **Mode-internal search:** `/` search within log mode is independent from the
  document Search mode. Follows less/vim conventions for familiarity.
