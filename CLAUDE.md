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
Markdown → convert.rs → Typst markup
  → render.rs: compile_document() → PagedDocument
  → tile.rs: split_frame() → Vec<Frame> (vertical tiles)
  → TiledDocument: lazy per-tile PNG rendering with LRU cache
  → viewer.rs: Kitty Graphics Protocol display + scroll
```

The document is compiled once with `height: auto` (single tall page), then the Frame tree
is split into vertical tiles. Only visible tiles are rendered to PNG on demand,
keeping peak memory proportional to tile size, not document size.

## Key Files

- `src/lib.rs` — Public module declarations (convert, render, tile, viewer, world)
- `src/main.rs` — CLI entry point (clap subcommands: default=viewer, `render`=PNG output)
- `src/convert.rs` — Markdown→Typst conversion (pulldown-cmark event handler, 20 unit tests inline)
- `src/world.rs` — Typst World trait implementation (virtual filesystem for typst compiler)
- `src/render.rs` — Typst compile (`compile_document`) + tile PNG render (`render_frame_to_png`) + debug dump
- `src/tile.rs` — Tile-based document model: frame splitting, visual line extraction, lazy rendering, viewport calculation
- `src/viewer.rs` — Terminal viewer using Kitty Graphics Protocol
- `themes/catppuccin.typ` — Default dark theme (Catppuccin Mocha)
- `tests/integration.rs` — Integration tests (fixture-based: load md → convert → render → verify PNG)
- `tests/fixtures/` — Test Markdown files
- `docs/typst-api-notes.md` — Typst 0.14.2 API reference notes
- `docs/paged-document-structure.md` — PagedDocument frame tree structure (実機ダンプ付き)
- `docs/kitty-graphics-protocol.md` — Kitty Graphics Protocol full spec reference
- `docs/terminal-viewer-design.md` — Terminal viewer design decisions and architecture
- `docs/visual-line-extraction.md` — フレームツリーからの視覚行抽出とベースラインオフセット問題

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

# Test (26 unit tests in convert.rs + 17 integration tests)
cargo test

# Run a single test
cargo test <test_name>

# Check
cargo check

# Debug logging (viewer/tile internals)
RUST_LOG=debug cargo run -- <input.md>

# Fuzz testing (requires nightly-2025-11-01; newer nightlies may break libfuzzer-sys)
cargo +nightly-2025-11-01 fuzz run fuzz_convert -- -max_total_time=30
cargo +nightly-2025-11-01 fuzz run fuzz_pipeline -- -max_total_time=30
```

## Dependencies

- typst 0.14, typst-render 0.14, typst-kit 0.14 (features: embed-fonts)
- pulldown-cmark 0.12 (options: ENABLE_TABLES, ENABLE_STRIKETHROUGH)
- clap 4 (derive)
- crossterm 0.28, base64 0.22 (for viewer)
- comemo 0.5, ecow 0.2 (typst lazy hashing / compact strings)
- anyhow 1 (error handling), log 0.4 + env_logger 0.11
- Rust edition 2024

## Implementation Status

- Phase 1 (complete): Paragraph text only. Japanese typography verified.
- Phase 2 (complete): Block elements (headings, code blocks, tables, lists, quotes)
- Phase 3 (planned): Edge cases and conversion quality
- Phase 4 (in progress): Kitty Graphics Protocol terminal display
  - Tile-based lazy rendering, visual line extraction, sidebar line numbers, scroll — implemented
  - Design doc: `docs/terminal-viewer-design.md`
  - Protocol spec: `docs/kitty-graphics-protocol.md`

## Viewer Constants

- PPI = 144.0 (fixed in viewer, configurable in `render` CLI)
- Tile height = 500pt minimum (at least viewport height)
- Scroll step = 3 terminal cells per j/k press
- Frame budget = 32ms (~30fps)
- Sidebar = 6 columns (fixed)
- Kitty Protocol: all commands use `q=2` to suppress responses (avoids crossterm misparse)

## Notes

- Theme is inlined into main.typ (not #include) because set-rule propagation
  didn't work with separate virtual files in typst 0.14.2
- IPAGothic is the primary CJK font; Noto Sans CJK JP as fallback
- FontBook uses lowercased family names for lookups
- The `typst/` directory contains typst source for API reference only (not used as dependency)
- typst-kit feature name is `embed-fonts` (not `embedded-fonts`)
- convert.rs uses a Container enum + stack for nested markup state tracking
- Viewer outer loop handles resize (rebuilds document); inner loop (thread::scope) handles scrolling with prefetch worker
