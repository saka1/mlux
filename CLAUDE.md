# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

tmark — Markdown to PNG renderer using Rust + Typst. Parses Markdown with pulldown-cmark,
converts to Typst markup, compiles via Typst's engine, and renders to PNG.
Includes a terminal viewer (`tview`) using Kitty Graphics Protocol with strip-based lazy rendering.

## Architecture

### CLI pipeline (`tmark`)

```
Markdown → pulldown-cmark → Event stream → convert.rs → Typst markup
  → world.rs (single main.typ: theme + width + content)
  → render.rs: typst::compile → PagedDocument → typst_render → PNG
```

### Terminal viewer pipeline (`tview`)

```
Markdown → convert.rs → Typst markup
  → render.rs: compile_document() → PagedDocument
  → strip.rs: split_frame() → Vec<Frame> (vertical strips)
  → StripDocument: lazy per-strip PNG rendering with LRU cache
  → tview.rs: Kitty Graphics Protocol display + scroll
```

The document is compiled once with `height: auto` (single tall page), then the Frame tree
is split into vertical strips. Only visible strips are rendered to PNG on demand,
keeping peak memory proportional to strip size, not document size.

## Key Files

- `src/main.rs` — CLI entry point (clap)
- `src/convert.rs` — Markdown→Typst conversion (pulldown-cmark event handler)
- `src/world.rs` — Typst World trait implementation (virtual filesystem for typst compiler)
- `src/render.rs` — Typst compile + PNG render + debug dump utilities
- `src/strip.rs` — Strip-based document model: frame splitting, visual line extraction, lazy rendering, viewport calculation
- `src/bin/tview.rs` — Terminal viewer using Kitty Graphics Protocol
- `themes/catppuccin.typ` — Default dark theme (Catppuccin Mocha)
- `tests/integration.rs` — Integration tests
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

# Run CLI
cargo run -- <input.md> -o <output.png> [--width 800] [--ppi 144] [--theme catppuccin]

# Run terminal viewer (requires Kitty Graphics Protocol support, e.g. Ghostty/Kitty)
cargo run --bin tview -- <input.md>

# Dump PagedDocument frame tree (debug)
cargo run -- <input.md> --dump

# Test (all 25 tests)
cargo test

# Run a single test
cargo test <test_name>

# Check
cargo check
```

## Dependencies

- typst 0.14, typst-render 0.14, typst-kit 0.14 (features: embed-fonts)
- pulldown-cmark 0.12
- clap 4 (derive)
- crossterm 0.28, base64 0.22 (for tview)
- Rust edition 2024

## Implementation Status

- Phase 1 (complete): Paragraph text only. Japanese typography verified.
- Phase 2 (complete): Block elements (headings, code blocks, tables, lists, quotes)
- Phase 3 (planned): Edge cases and conversion quality
- Phase 4 (in progress): Kitty Graphics Protocol terminal display
  - Strip-based lazy rendering, visual line extraction, sidebar line numbers, scroll — implemented
  - Design doc: `docs/terminal-viewer-design.md`
  - Protocol spec: `docs/kitty-graphics-protocol.md`

## Notes

- Theme is inlined into main.typ (not #include) because set-rule propagation
  didn't work with separate virtual files in typst 0.14.2
- IPAGothic is the primary CJK font; Noto Sans CJK JP as fallback
- FontBook uses lowercased family names for lookups
- The `typst/` directory contains typst source for API reference only (not used as dependency)
- typst-kit feature name is `embed-fonts` (not `embedded-fonts`)
