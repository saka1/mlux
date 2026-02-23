# tmark — Markdown→PNG Renderer via Typst

## Project Overview

Markdown to PNG renderer using Rust + Typst. Parses Markdown with pulldown-cmark,
converts to Typst markup, compiles via Typst's engine, and renders to PNG.

## Architecture

```
Markdown → pulldown-cmark → Event stream → convert.rs → Typst markup
  → world.rs (single main.typ: theme + width + content)
  → typst::compile::<PagedDocument>()
  → typst_render::render()
  → PNG
```

## Key Files

- `src/main.rs` — CLI entry point (clap)
- `src/lib.rs` — Module declarations
- `src/convert.rs` — Markdown→Typst conversion
- `src/world.rs` — Typst World trait implementation
- `src/render.rs` — Compile + render to PNG
- `themes/catppuccin.typ` — Default dark theme (Catppuccin Mocha)
- `tests/integration.rs` — Integration tests
- `tests/fixtures/` — Test Markdown files
- `docs/typst-api-notes.md` — Typst 0.14.2 API reference notes
- `docs/paged-document-structure.md` — PagedDocument frame tree structure (実機ダンプ付き)
- `docs/kitty-graphics-protocol.md` — Kitty Graphics Protocol full spec reference
- `docs/terminal-viewer-design.md` — Terminal viewer design decisions and architecture

## Commands

```bash
# Build
cargo build

# Run
cargo run -- <input.md> -o <output.png> [--width 800] [--ppi 144] [--theme catppuccin]

# Dump PagedDocument frame tree (debug)
cargo run -- <input.md> --dump

# Test
cargo test

# Check
cargo check
```

## Dependencies

- typst 0.14, typst-render 0.14, typst-kit 0.14 (features: embed-fonts)
- pulldown-cmark 0.12
- clap 4 (derive)
- Rust edition 2024

## Implementation Status

- Phase 1 (complete): Paragraph text only. Japanese typography verified.
- Phase 2 (complete): Block elements (headings, code blocks, tables, lists, quotes)
- Phase 3 (planned): Edge cases and conversion quality
- Phase 4 (planned): Kitty Graphics Protocol terminal display
  - Design doc: `docs/terminal-viewer-design.md`
  - Protocol spec: `docs/kitty-graphics-protocol.md`

## Notes

- Theme is inlined into main.typ (not #include) because set-rule propagation
  didn't work with separate virtual files in typst 0.14.2
- IPAGothic is the primary CJK font; Noto Sans CJK JP as fallback
- FontBook uses lowercased family names for lookups
- The `typst/` directory contains typst source for API reference only (not used as dependency)
