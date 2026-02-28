# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

mlux — Markdown to PNG renderer using Rust + Typst. Parses Markdown with pulldown-cmark,
converts to Typst markup, compiles via Typst's engine, and renders to PNG.
Includes a terminal viewer using Kitty Graphics Protocol with tile-based lazy rendering.

## Architecture

### Render pipeline (`mlux render`)

```
Markdown → pulldown-cmark → Event stream → convert.rs → Typst markup
  → world.rs (single main.typ: theme + width + content)
  → render.rs: typst::compile → PagedDocument → typst_render → PNG
```

### Terminal viewer pipeline (`mlux <file>`)

```
Markdown → convert.rs → Typst markup (with SourceMap)
  → render.rs: compile_document() → PagedDocument
  → tile.rs: split_frame() → Vec<Frame> (vertical tiles)
  → TiledDocument: lazy per-tile PNG rendering with LRU cache
  → viewer/: Kitty Graphics Protocol display + scroll + modes
```

The document is compiled once with `height: auto` (single tall page), then the Frame tree
is split into vertical tiles. Only visible tiles are rendered to PNG on demand,
keeping peak memory proportional to tile size, not document size.

### Viewer architecture (`src/viewer/`)

The viewer uses a two-loop architecture:

- **Outer loop** (`mod.rs`): Handles document lifecycle — initial build, resize (rebuilds
  with new dimensions), file reload (re-reads markdown), and config reload.
  Font cache is shared across iterations.
- **Inner loop** (`mod.rs` within `thread::scope`): Handles input events, scrolling,
  and mode transitions. Runs a prefetch worker thread that renders adjacent tiles
  in the background via mpsc channels.

Modal input handling with Effect pattern:

- **Modes**: `Normal` (tile display + vim-style navigation), `Search` (heading picker),
  `Command` (`:` prompt for `:reload`, `:quit`)
- **Effect pattern**: Mode handlers return `Vec<Effect>` (ScrollTo, Flash, SetMode, Yank, etc.)
  which the apply loop in `run()` executes. This separates "what to do" from "how to do it".
- **Input**: `InputAccumulator` in `input.rs` handles vim-style number prefix accumulation
  (e.g., `10j`, `56g`, `56y`).

Submodule responsibilities:
- `mod.rs` — Main run loop, Effect enum, effect application
- `state.rs` — Layout calculation, ViewState, redraw, prefetch dispatch
- `input.rs` — Key event mapping, Action enum, InputAccumulator
- `terminal.rs` — Raw mode guard, Kitty Graphics Protocol commands, status bar
- `pipeline.rs` — Markdown → TiledDocument build (bridges viewer layout to tile.rs)
- `mode_normal.rs` — Normal mode handler (scroll, jump, yank, mode transitions)
- `mode_search.rs` — Search/heading picker mode
- `mode_command.rs` — Command mode (`:reload`, `:quit`)

### Config system (`src/config.rs`)

Two-layer config: `ConfigFile` (TOML deserialization, all `Option`) → `Config` (resolved
with defaults). Config loaded from `~/.config/mlux/config.toml` (respects `$XDG_CONFIG_HOME`).
CLI args override config via `CliOverrides`, which are preserved across live config reloads.

### Source mapping (`convert.rs` → `tile.rs`)

`markdown_to_typst_with_map()` produces both Typst markup and a `SourceMap` that maps
Typst byte offsets back to Markdown line numbers. `tile.rs` uses this to annotate each
`VisualLine` with `md_line_range` for sidebar line numbers and yank operations.

## Key Files

- `src/convert.rs` — Markdown→Typst conversion (pulldown-cmark event handler, Container enum + stack for nested markup)
- `src/world.rs` — Typst World trait implementation (virtual filesystem for typst compiler)
- `src/render.rs` — Typst compile (`compile_document`) + tile PNG render (`render_frame_to_png`) + debug dump
- `src/tile.rs` — Tile-based document model: frame splitting, visual line extraction, lazy rendering, viewport calculation
- `src/viewer/` — Terminal viewer (see Viewer architecture above)
- `src/config.rs` — Config file loading, CLI override merging, default resolution
- `src/watch.rs` — File change detection via `notify` crate (inotify on Linux)
- `themes/catppuccin.typ` — Default dark theme (Catppuccin Mocha)
- `tests/integration.rs` — Integration tests (fixture-based: load md → convert → render → verify PNG)
- `docs/terminal-viewer-design.md` — Terminal viewer design decisions and architecture
- `docs/kitty-graphics-protocol.md` — Kitty Graphics Protocol full spec reference

## Commands

```bash
# Build
cargo build

# View in terminal (default mode, requires Kitty Graphics Protocol support)
cargo run -- <input.md>

# Render to PNG tiles (outputs output-000.png, output-001.png, ...)
cargo run -- render <input.md> -o <output.png> [--width 800] [--ppi 144] [--tile-height 500] [--theme catppuccin]

# Dump PagedDocument frame tree (debug)
cargo run -- render <input.md> --dump

# Test (71 unit tests + 18 integration tests)
cargo test

# Run a single test
cargo test <test_name>

# Check
cargo check

# Lint (clippy) — must stay warning-free
cargo clippy

# Debug logging (writes to file to avoid corrupting terminal UI)
cargo run -- --log /tmp/mlux.log <input.md>

# Fuzz testing (requires nightly-2025-11-01; newer nightlies may break libfuzzer-sys)
cargo +nightly-2025-11-01 fuzz run fuzz_convert -- -max_total_time=30
cargo +nightly-2025-11-01 fuzz run fuzz_pipeline -- -max_total_time=30
```

## Quality Gates

After any code change, ensure all pass:

```bash
cargo fmt       # auto-format (or `cargo fmt --check` to verify)
cargo clippy    # zero warnings
cargo test      # all tests pass
```

## Dependencies

- typst 0.14, typst-render 0.14, typst-kit 0.14 (features: embed-fonts)
- pulldown-cmark 0.12 (options: ENABLE_TABLES, ENABLE_STRIKETHROUGH)
- clap 4 (derive), serde 1 + toml 0.8 (config)
- crossterm 0.28, base64 0.22 (viewer terminal I/O)
- notify 8 (file watching via inotify)
- comemo 0.5, ecow 0.2 (typst lazy hashing / compact strings)
- anyhow 1 (error handling), log 0.4 + env_logger 0.11
- Rust edition 2024

## Implementation Status

- Phase 1 (complete): Paragraph text only. Japanese typography verified.
- Phase 2 (complete): Block elements (headings, code blocks, tables, lists, quotes)
- Phase 3 (planned): Edge cases and conversion quality
- Phase 4 (in progress): Kitty Graphics Protocol terminal display
  - Tile-based lazy rendering, visual line extraction, sidebar line numbers, scroll — implemented
  - Vim-style modes (normal/search/command), file watching, config reload — implemented
  - Design doc: `docs/terminal-viewer-design.md`
  - Protocol spec: `docs/kitty-graphics-protocol.md`

## Viewer Constants

All configurable via `~/.config/mlux/config.toml` (defaults shown):

- PPI = 144.0, Width = 660pt
- Tile height = 500pt minimum (at least viewport height)
- Scroll step = 3 terminal cells per j/k press
- Frame budget = 32ms (~30fps)
- Sidebar = 6 columns
- Kitty Protocol: all commands use `q=2` to suppress responses (avoids crossterm misparse)

## Notes

- Theme is inlined into main.typ (not #include) because set-rule propagation
  didn't work with separate virtual files in typst 0.14.2
- IPAGothic is the primary CJK font; Noto Sans CJK JP as fallback
- FontBook uses lowercased family names for lookups
- The `typst/` directory contains typst source for API reference only (not used as dependency)
- typst-kit feature name is `embed-fonts` (not `embedded-fonts`)
- convert.rs uses a Container enum + stack for nested markup state tracking
- File watcher monitors the parent directory (not the file itself) because Linux inotify
  loses the watch handle on atomic-save (rename)
