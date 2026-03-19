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
- `LogEntry`: `{ timestamp: SystemTime, level: Level, target: String, message: String }`
  (`SystemTime` for wall-clock `HH:MM:SS.mmm` display)
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

- Always initialize `RingLog` — both viewer and render modes
- `env_logger` dependency removed; replaced by `RingLog` + `env_filter`
- `--debug` CLI flag added
- `Arc` clone of ring buffer passed to viewer
- `render` subcommand: same `RingLog` init (buffer exists but unused since no
  viewer; file output still works via `--log`)

### Log Viewer Mode (`src/viewer/mode_log.rs`)

New `ViewerMode::Log(LogState)` variant.

**LogState:**
- `scroll_offset: usize`
- `search_query: Option<String>` — active `/` search
- `search_matches: Vec<usize>` — matching line indices
- `search_index: usize` — current match position

**Display:**
- Same text-overlay approach as Toc/Search modes (ClearScreen + text drawing,
  no Kitty image tiles)
- Each line: `[HH:MM:SS.mmm] [LEVEL] target: message`
- Color by level: ERROR=red, WARN=yellow, INFO=default, DEBUG=dim
- Search matches highlighted
- Buffer contents are snapshot-cloned under lock, then lock released before
  terminal I/O

**Keymap:** `LogAction` enum + `map_log_key()` in `keymap.rs` (follows existing
per-mode pattern: `SearchAction`/`map_search_key`, `TocAction`/`map_toc_key`, etc.)

| Key          | Action                           |
|--------------|----------------------------------|
| `j/k`, arrows | scroll up/down                 |
| `g/G`        | top/bottom                       |
| `/`          | enter search (incremental, regex) |
| `n/N`        | next/previous match              |
| `y`          | yank all log text to clipboard   |
| `q/Esc`      | return to Normal mode (not quit) |

**Effects:** `Effect::RedrawLog` added for log mode repaints (parallels
`RedrawSearch`, `RedrawToc`, etc.)

**Entry point:**
- `:log` command in `mode_command.rs` → `Effect::SetMode(ViewerMode::Log(...))`

### Ring Buffer Ownership

The `Arc<Mutex<VecDeque<LogEntry>>>` is stored on `Session` (not `ViewContext`).
Session persists across document rebuilds (resize/reload), so the log buffer
survives outer-loop iterations. `ViewContext` borrows from Session per-event.

### Integration Points

| File                           | Change                                             |
|--------------------------------|----------------------------------------------------|
| `src/log.rs` (new)             | `RingLog`, `LogEntry`, ring buffer, `Log` impl     |
| `src/main.rs`                  | Always init logger, `--debug` flag                 |
| `src/viewer/effect.rs`         | `ViewerMode::Log(LogState)`, `Effect::RedrawLog`   |
| `src/viewer/mode_log.rs` (new) | `LogAction`, mode handler, display, scroll, search |
| `src/viewer/mode_command.rs`   | `:log` command                                     |
| `src/viewer/mod.rs`            | Mode routing for Log, Session gets buffer ref      |
| `src/viewer/keymap.rs`         | `LogAction` enum, `map_log_key()`                  |

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
- **`q` exits mode, not app:** Consistent with Toc/UrlPicker modes where `q`
  returns to Normal rather than quitting the application.
