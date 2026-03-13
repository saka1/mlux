# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

mlux — Markdown to PNG renderer using Rust + Typst. Parses Markdown with pulldown-cmark,
converts to Typst markup, compiles via Typst's engine, and renders to PNG.
Includes a terminal viewer using Kitty Graphics Protocol with tile-based lazy rendering.

## Architecture

### Render pipeline (`mlux render`)

```
Markdown → pulldown-cmark → Event stream
  → diagram.rs: extract mermaid blocks → render to SVG (mermaid-rs-renderer)
  → pipeline/markup.rs → Typst markup (mermaid → #image(), others → code fence)
  → pipeline/world.rs (single main.typ: theme + width + content + SVG images)
  → pipeline/render.rs: typst::compile → PagedDocument → typst_render → PNG
```

### Terminal viewer pipeline (`mlux <file>`)

```
Markdown → build_tiled_document() orchestrates full pipeline:
  → image.rs: extract_image_paths → load_images
  → diagram.rs: extract_diagrams → render_diagrams (mermaid → SVG)
  → pipeline/markup.rs → Typst markup (with SourceMap)
  → pipeline/render.rs: compile_document() → PagedDocument
  → tile.rs: split_frame() → Vec<Frame> (vertical tiles)
  → TiledDocument: lazy per-tile PNG rendering with LRU cache
  → viewer/: Kitty Graphics Protocol display + scroll + modes
```

The document is compiled once with `height: auto` (single tall page), then the Frame tree
is split into vertical tiles. Only visible tiles are rendered to PNG on demand.

### Viewer design

- **Two-loop**: Outer loop handles document lifecycle (build/resize/reload).
  Inner loop (in `thread::scope`) handles input, scrolling, mode transitions, and prefetch worker.
- **Effect pattern**: Mode handlers return `Vec<Effect>` — the apply loop in `run()` executes them.
- **Modes**: `Normal` (vim-style navigation), `Search` (heading picker), `Command` (`:reload`, `:quit`)
- **Source mapping**: `markdown_to_typst()` produces a `SourceMap` (Typst byte offset → Markdown line).
  `tile.rs` uses this for sidebar line numbers and yank operations.

### Config system (`src/config.rs`)

Two-layer: `ConfigFile` (TOML, all `Option`) → `Config` (resolved with defaults).
CLI args override config via `CliOverrides`, preserved across live config reloads.

## Commands

```bash
cargo build
cargo run -- <input.md>                    # terminal viewer (Kitty Graphics Protocol)
cargo run -- render <input.md> -o out.png  # render to PNG [--width 800] [--ppi 144] [--theme catppuccin]
cargo run -- render <input.md> --dump      # dump frame tree (debug)
cargo run -- --log /tmp/mlux.log <input.md>  # debug logging
cargo test                                 # all tests
cargo test <test_name>                     # single test

# Fuzz (requires nightly-2025-11-01)
cargo +nightly-2025-11-01 fuzz run fuzz_convert -- -max_total_time=30
cargo +nightly-2025-11-01 fuzz run fuzz_pipeline -- -max_total_time=30
```

## Quality Gates

After any code change, ensure all pass:

```bash
cargo fmt       # auto-format
cargo clippy    # zero warnings
cargo test      # all tests pass
```

## Non-obvious Notes

- Theme is inlined into main.typ (not #include) — set-rule propagation doesn't work with separate virtual files in typst 0.14
- File watcher monitors the parent directory, not the file itself — Linux inotify loses watch handle on atomic-save (rename)
- `pipeline/convert.rs` uses a Container enum + stack for nested markup state tracking
- typst-kit feature name is `embed-fonts` (not `embedded-fonts`)
- FontBook uses lowercased family names for lookups
- Kitty Protocol commands use `q=2` to suppress responses (avoids crossterm misparse)
- Rust edition 2024
