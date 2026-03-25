# Latin Mode (欧文モード) Design

## Overview

Automatic detection of CJK content in Markdown documents, switching to a dedicated
Latin/European theme variant when no CJK characters are present. This enables
independent typographic tuning (font, italic, spacing) for Latin-only documents.

## Motivation

- Noto Sans JP has no true italic variant; Japanese fonts generally lack italic styles
- Latin-only documents should benefit from dedicated typographic settings automatically
- Separate theme files allow CJK and Latin themes to evolve independently
  (font choice, italic support, letter-spacing, line-height, etc.)
- Font selection for the latin theme is deferred to a future PR

## Key architectural insight

**To build AppContext, you must first parse the markdown.**

Currently AppContext is constructed before markdown is read, and the theme is fully
resolved at that point. Latin mode breaks this assumption: the theme depends on
document content (`has_cjk`). This requires reordering the initialization sequence.

### Design decision: prescan once, fixed for process lifetime

Prescan runs **once at startup**. The `has_cjk` result is fixed for the entire process
lifetime — even if the file changes on disk and the viewer reloads, the CJK/latin
determination does not change. This dramatically simplifies the design:

- No fork topology questions (prescan runs inline before AppContext, not in fork sandbox)
- No viewer reload propagation (the value never changes)
- No re-resolution of theme on rebuild

pulldown-cmark parsing is lightweight and processes trusted local input, so running it
outside a fork sandbox is acceptable.

### New initialization flow

```
main.rs:
  config, cli_overrides, fonts, detected_light
  input_source → markdown = read_all()
  prescan(&markdown) → Prescan { image_paths, has_cjk }
  AppContextBuilder + has_cjk → AppContext (theme resolved including latin)
  cmd_render(app, input) / viewer::run(app, input)
```

Compared to the current flow, the only structural change is: markdown is read and
prescanned before `AppContextBuilder::build()`.

## Detection: Prescan phase

### Rename and extend `extract_image_paths`

The existing `extract_image_paths()` in `src/pipeline/markup.rs` performs a lightweight
pulldown-cmark parse to collect image URLs. This function is reframed as a general
**prescan phase** that collects multiple pieces of metadata in a single pass.

```rust
// src/pipeline/markup.rs

/// Information collected from a lightweight pre-scan of the Markdown source.
pub struct Prescan {
    /// Image paths referenced in the document (deduplicated).
    pub image_paths: Vec<String>,
    /// Whether the document contains any CJK characters.
    pub has_cjk: bool,
}

/// Pre-scan Markdown source to collect metadata without full conversion.
pub fn prescan(markdown: &str) -> Prescan {
    // Existing pulldown-cmark event loop, extended:
    // - Image/Html events: collect image paths (as before)
    // - Text events: check for CJK characters
}
```

`Prescan` derives `Serialize`/`Deserialize` for the fork sandbox boundary in
`usecase.rs` (Fork 1 still runs prescan for its image path extraction role).

### CJK detection algorithm

Primitive / v1: any single CJK character triggers CJK mode.

```rust
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{9FFF}'   // CJK Unified + Hiragana, Katakana, symbols
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth/Fullwidth Forms
    )
}
```

The check runs on `Text` events during the prescan pulldown-cmark walk. Once a CJK
character is found, `has_cjk` is set to `true` and further text scanning is skipped
(short-circuit).

Known limitation: `\u{3000}` (ideographic space) and other CJK punctuation in the
range will trigger CJK mode. Acceptable for v1.

## Theme variant system

### Approach: separate theme files

Each built-in theme gets a `-latin` variant as an independent `.typ` file.

| Theme | CJK version | Latin version |
|-------|-------------|---------------|
| Catppuccin Mocha (dark) | `catppuccin.typ` | `catppuccin-latin.typ` |
| Catppuccin Latte (light) | `catppuccin-latte.typ` | `catppuccin-latte-latin.typ` |

Latin variants share all color definitions, sidebar colors, mermaid colors, and
`data_files` (`.tmTheme` for syntax highlighting) with their CJK counterpart.
The differences are:

- Body font: currently `"Noto Sans JP"` (placeholder — dedicated Latin font TBD in future PR)
- Latin themes exist as independent files that can diverge from CJK themes over time

### Theme resolution

`resolve_theme_name()` gains `has_cjk: bool`:

```rust
pub fn resolve_theme_name(name: &str, is_light: bool, has_cjk: bool) -> &str
```

All three axes (dark/light, CJK/latin, alias/explicit) are resolved in a single call.

Behavior:
- **Alias names** (`auto`, `dark`, `light`): resolve dark/light, then apply `-latin`
  suffix when `!has_cjk`
- **Explicit theme names** (`catppuccin`, `catppuccin-latin`, etc.): passed through
  unchanged — user intent is respected

### ThemeEntry additions

Latin variants are added as flat entries in the `THEMES` array, each with their own
`ThemeEntry`. The `sidebar_bg`, `sidebar_fg`, `mermaid`, and `data_files` fields are
identical to the base theme.

### Latin variant mapping

A static mapping connects base themes to their latin variants:

```rust
const LATIN_VARIANTS: &[(&str, &str)] = &[
    ("catppuccin", "catppuccin-latin"),
    ("catppuccin-latte", "catppuccin-latte-latin"),
];
```

## Initialization sequence changes

### `main.rs`

```
// Before (current):
let app = AppContextBuilder::new(config, cli_overrides)
    .load_fonts().set_detected_light(detected_light).build()?;
let input_source = build_input_source(...);
cmd_render(app, input_source, ...) / viewer::run(app, input_source, ...)

// After:
let mut input_source = build_input_source(...);
let markdown = input_source.read_all()?;        // read once, early
let prescan = pipeline::prescan(&markdown);      // inline, no fork
let app = AppContextBuilder::new(config, cli_overrides)
    .load_fonts().set_detected_light(detected_light)
    .set_has_cjk(prescan.has_cjk)               // new
    .build()?;
cmd_render(app, markdown, ...) / viewer::run(app, markdown, ...)
```

`AppContextBuilder` gains `set_has_cjk(bool)`. Default is `true` (CJK mode = current
behavior) if not set.

Markdown is read once in `main.rs` and passed as `String` to `cmd_render` / `viewer::run`.
These functions no longer call `input.read_all()` themselves. For the viewer, the initial
markdown is passed in; subsequent reloads read from the file path (stdin input does not
support reload).

### `usecase.rs` fork path

Fork 1 changes from `extract_image_paths` to `prescan`, returning `Prescan`.
The parent uses `prescan.image_paths` for image loading. `prescan.has_cjk` is
available but not used here (the startup prescan already determined the theme).

### Viewer reload

No changes needed for `has_cjk`. The theme was resolved at startup and remains
fixed. The viewer's config reload path (`resolve_theme_name` in `viewer/mod.rs`)
gains the `has_cjk` parameter, sourced from `app.has_cjk` (a new field on `AppContext`).

## Feature rename

`embed-noto-fonts` → `embed-fonts`

The feature gates embedding of all fonts in `fonts/` (currently: Noto Sans JP,
STIX Two Math). The old name implied only Noto was embedded; the generic name
accommodates future font additions.

## Files changed

| File | Change |
|------|--------|
| `src/main.rs` | Read markdown + prescan before AppContext; pass markdown to cmd_render/viewer |
| `src/app_context.rs` | `AppContextBuilder::set_has_cjk()`, store `has_cjk` on `AppContext` |
| `src/pipeline/markup.rs` | `extract_image_paths` → `prescan`, add `Prescan` struct |
| `src/pipeline/mod.rs` | Update re-exports |
| `src/pipeline/build.rs` | Use `prescan()` in non-fork paths |
| `src/usecase.rs` | Fork 1: `prescan()` instead of `extract_image_paths()` |
| `src/viewer/mod.rs` | Pass `app.has_cjk` to `resolve_theme_name` in config reload |
| `src/theme.rs` | `resolve_theme_name` gains `has_cjk`, add latin `ThemeEntry`s, `LATIN_VARIANTS` |
| `themes/catppuccin-latin.typ` | New: Latin variant dark theme |
| `themes/catppuccin-latte-latin.typ` | New: Latin variant light theme |
| `Cargo.toml` | Rename feature `embed-noto-fonts` → `embed-fonts` |
| `build.rs` | Update feature gate env var |
| `tests/integration.rs` | Update `extract_image_paths` calls |

## Files NOT changed

- `src/config.rs` — no new config fields (auto-detection, no user setting needed)
- `src/pipeline/world.rs` / `FontCache` — font loading unchanged
- CLI arguments — no additions

## Open questions

- Latin font selection: choose a dedicated Latin font (e.g. Inter, Libertinus)
  and embed it. Currently using Noto Sans JP as placeholder.
- Italic rendering: once a Latin font with italic variant is selected, verify
  Typst auto-selects it for `#emph`
