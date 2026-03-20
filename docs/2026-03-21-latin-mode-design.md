# Latin Mode (欧文モード) Design

## Overview

Automatic detection of CJK content in Markdown documents, switching to an optimized
Latin/European font theme (Inter) when no CJK characters are present. This enables
true italic rendering and better typographic quality for English/European-language documents.

## Motivation

- Noto Sans JP has no true italic variant; Japanese fonts generally lack italic styles
- Inter provides Regular, Italic, Bold, BoldItalic — full typographic coverage for Latin text
- Documents without CJK content should benefit from a dedicated Latin font automatically

## Key architectural insight

**To build AppContext, you must first parse the markdown.**

Currently AppContext is constructed before markdown is read, and the theme is fully
resolved at that point. Latin mode breaks this assumption: the theme depends on
document content (`has_cjk`). This requires reordering the initialization sequence.

### Current flow

```
main.rs:
  config → AppContextBuilder → AppContext (theme fully resolved)
  input_source
  cmd_render(app, input) / viewer::run(app, input)

cmd_render():
  markdown = input.read_all()
  app.build_params(markdown, ...) → BuildParams

usecase.rs (fork path):
  Fork 1: extract_image_paths(markdown)   [sandboxed]
  Parent:  fetch remote images
  Fork 2:  compile + render               [sandboxed]
```

### New flow

```
main.rs:
  config, cli_overrides, fonts, detected_light   ← prepare, but don't build AppContext yet
  input_source → markdown = read_all()
  Fork 0: prescan(markdown)                      [sandboxed, lightweight]
  AppContextBuilder + has_cjk → AppContext        (theme resolved with latin knowledge)
  cmd_render(app, markdown) / viewer::run(app, markdown)

usecase.rs (fork path):
  Fork 1: prescan(markdown)  → Prescan           [sandboxed — replaces extract_image_paths]
  Parent:  fetch remote images using prescan.image_paths
  Fork 2:  compile + render                      [sandboxed]
```

Note: in the non-fork code paths (`build_tiled_document`, `build_and_dump`), prescan
runs directly (no fork) since these are used in tests and simple render mode.

### Viewer reload path

When the viewer reloads (file change or `:reload`), the markdown is re-read and the
document is rebuilt. Prescan must run on the new markdown content. If `has_cjk` changes
(e.g. user removed all Japanese text), the theme must be re-resolved.

The viewer's outer loop already re-reads markdown and rebuilds the document. The prescan
result feeds into theme re-resolution at this point. `resolve_theme_name()` in
`viewer/mod.rs` (config reload path) gains `has_cjk` awareness.

## Detection: Prescan phase

### Rename and extend `extract_image_paths`

The existing `extract_image_paths()` in `src/pipeline/markup.rs` performs a lightweight
pulldown-cmark parse to collect image URLs. This function is reframed as a general
**prescan phase** that collects multiple pieces of metadata in a single pass.

```rust
// src/pipeline/markup.rs

/// Information collected from a lightweight pre-scan of the Markdown source.
#[derive(Serialize, Deserialize)]
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

## Theme variant system

### Approach: separate theme files

Each built-in theme gets a `-latin` variant as an independent `.typ` file.

| Theme | CJK version | Latin version |
|-------|-------------|---------------|
| Catppuccin Mocha (dark) | `catppuccin.typ` | `catppuccin-latin.typ` |
| Catppuccin Latte (light) | `catppuccin-latte.typ` | `catppuccin-latte-latin.typ` |

Latin variants share all color definitions, sidebar colors, and mermaid colors with
their CJK counterpart. The differences are:

- Body font: `"Noto Sans JP"` → `"Inter"`
- Code font fallback: `("DejaVu Sans Mono", "Noto Sans JP")` → `"DejaVu Sans Mono"`
  (or keep Noto as distant fallback — TBD based on output quality)
- Italic: Typst's font matching selects Inter-Italic automatically for `#emph`

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
`ThemeEntry`. The `sidebar_bg`, `sidebar_fg`, and `mermaid` fields are identical to
the base theme.

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

Current:
```
AppContextBuilder::new(config, cli_overrides).load_fonts().set_detected_light(...).build()
→ cmd_render(app, input_source) / viewer::run(app, input_source)
```

New:
```
let partial = AppContextBuilder::new(config, cli_overrides).load_fonts().set_detected_light(...);
let markdown = input_source.read_all();   // read markdown before AppContext
let prescan = prescan(&markdown);         // or fork_compute(prescan) for sandbox
let app = partial.set_has_cjk(prescan.has_cjk).build();
→ cmd_render(app, markdown, prescan) / viewer::run(app, markdown, prescan)
```

`AppContextBuilder` gains a `set_has_cjk(bool)` method. `build()` uses it in
`resolve_theme_name()`. Default is `true` (CJK mode = current behavior) if not set.

### `cmd_render()`

Receives `markdown: String` and `prescan: Prescan` instead of reading markdown itself.
Uses `prescan.image_paths` for image loading (replaces `extract_image_paths` call).

### Viewer

The viewer reads markdown in its outer loop. On each iteration (initial + reload),
prescan runs on the fresh markdown. If `has_cjk` differs from the previous build,
the theme is re-resolved.

### `usecase.rs` fork path

Fork 1 changes from `extract_image_paths` to `prescan`, returning `Prescan`
(which implements `Serialize`/`Deserialize`). The parent uses both `image_paths`
and `has_cjk` from the result.

## Feature rename

`embed-noto-fonts` → `embed-fonts`

The feature gates embedding of all fonts in `fonts/` (currently: Noto Sans JP, Inter,
STIX Two Math). The old name implied only Noto was embedded.

Changes:
- `Cargo.toml`: rename feature and update `default`
- `build.rs`: `CARGO_FEATURE_EMBED_NOTO_FONTS` → `CARGO_FEATURE_EMBED_FONTS`

## Inter font embedding

The four Inter font files are already in `fonts/`:
- `Inter-Regular.ttf`
- `Inter-Italic.ttf`
- `Inter-Bold.ttf`
- `Inter-BoldItalic.ttf`

License: SIL Open Font License 1.1 (`fonts/OFL-Inter.txt`). Permits bundling and
redistribution with software. No changes to `build.rs` needed — it already compresses
and embeds all `.ttf` files in `fonts/`.

## Files changed

| File | Change |
|------|--------|
| `src/main.rs` | Reorder: read markdown + prescan before AppContext build |
| `src/app_context.rs` | `AppContextBuilder::set_has_cjk()`, pass to `resolve_theme_name` |
| `src/pipeline/markup.rs` | `extract_image_paths` → `prescan`, add `Prescan` struct |
| `src/pipeline/mod.rs` | Update re-exports |
| `src/pipeline/build.rs` | Use `prescan()` in non-fork paths |
| `src/usecase.rs` | Fork 1: `prescan()` instead of `extract_image_paths()` |
| `src/viewer/mod.rs` | Prescan on each rebuild, re-resolve theme if `has_cjk` changes |
| `src/theme.rs` | `resolve_theme_name` gains `has_cjk`, add latin `ThemeEntry`s, `LATIN_VARIANTS` |
| `themes/catppuccin-latin.typ` | New: Inter-based dark theme |
| `themes/catppuccin-latte-latin.typ` | New: Inter-based light theme |
| `Cargo.toml` | Rename feature `embed-noto-fonts` → `embed-fonts` |
| `build.rs` | Update feature gate env var |
| `tests/integration.rs` | Update `extract_image_paths` calls |

## Files NOT changed

- `src/config.rs` — no new config fields (auto-detection, no user setting needed)
- `src/pipeline/world.rs` / `FontCache` — font loading unchanged
- CLI arguments — no additions

## Open questions

- Code block font in latin mode: drop Noto Sans JP fallback entirely, or keep as
  distant fallback? (Decide based on output quality inspection)
- Italic rendering: verify Typst auto-selects Inter-Italic for `#emph`. If not,
  add explicit `#show emph: set text(style: "italic")`
