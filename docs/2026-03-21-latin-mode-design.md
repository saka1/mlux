# Latin Mode (欧文モード) Design

## Overview

Automatic detection of CJK content in Markdown documents, switching to an optimized
Latin/European font theme (Inter) when no CJK characters are present. This enables
true italic rendering and better typographic quality for English/European-language documents.

## Motivation

- Noto Sans JP has no true italic variant; Japanese fonts generally lack italic styles
- Inter provides Regular, Italic, Bold, BoldItalic — full typographic coverage for Latin text
- Documents without CJK content should benefit from a dedicated Latin font automatically

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

### Affected call sites

| Call site | Current | New |
|-----------|---------|-----|
| `pipeline/build.rs: build_tiled_document()` | `extract_image_paths(&md)` | `prescan(&md)` |
| `pipeline/build.rs: build_and_dump()` | `extract_image_paths(&md)` | `prescan(&md)` |
| `usecase.rs: prepare_remote_images()` (fork sandbox) | `extract_image_paths(&md)` | `prescan(&md)` |
| `tests/integration.rs` | `extract_image_paths(&md)` | `prescan(&md).image_paths` |

The `Prescan` struct must implement `Serialize`/`Deserialize` (or be decomposed) for
the fork sandbox boundary in `usecase.rs`.

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

### Two-phase theme resolution

Theme resolution is split into two phases to handle the sequencing constraint:
`has_cjk` is not available when `AppContext` is built (before markdown is read).

**Phase 1 — dark/light axis** (`AppContext` build time, unchanged):
`resolve_theme_name()` resolves `auto`/`dark`/`light` aliases to a base theme name
(e.g. `catppuccin` or `catppuccin-latte`). This happens in `app_context.rs` and
`viewer/mod.rs` (config reload path). No changes to this phase.

**Phase 2 — latin axis** (`build_params()` or build pipeline):
After prescan produces `has_cjk`, the theme is re-resolved to its latin variant if
applicable. This happens in `AppContext::build_params()`, which already receives the
markdown string. The method runs prescan internally (or receives `Prescan` as argument)
and, when `!has_cjk`, looks up the `-latin` variant of the current theme.

```rust
// src/app_context.rs — build_params() updated
pub fn build_params(&self, markdown: String, ...) -> BuildParams {
    let prescan = crate::pipeline::prescan(&markdown);
    let (theme_name, theme_text, data_files) =
        if !prescan.has_cjk && self.theme_is_auto_resolved() {
            // Try latin variant (only for auto/dark/light aliases)
            theme::resolve_latin_variant(&self.theme.name)
                .unwrap_or((self.theme.name.clone(), self.theme.text, self.theme.data_files))
        } else {
            (self.theme.name.clone(), self.theme.text, self.theme.data_files)
        };
    BuildParams { theme_name, theme_text, data_files, markdown, ... }
}
```

A new helper `theme::resolve_latin_variant(base_name) -> Option<(String, &str, DataFiles)>`
maps a base theme to its latin variant (e.g. `"catppuccin"` → `"catppuccin-latin"`).
Returns `None` if no latin variant exists.

**Distinguishing auto vs explicit themes:** Latin auto-switching only activates when
the theme was resolved from an alias (`auto`, `dark`, `light`). This is determined by
comparing `self.config.theme` (the raw user/config value) against the known alias list.
If a user explicitly writes `--theme catppuccin`, Phase 2 does not switch to
`catppuccin-latin` — the user's explicit choice is respected.

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
| `src/pipeline/markup.rs` | `extract_image_paths` → `prescan`, add `Prescan` struct |
| `src/pipeline/mod.rs` | Update re-exports |
| `src/pipeline/build.rs` | Use `prescan()`, pass prescan results through pipeline |
| `src/app_context.rs` | `build_params()` runs prescan, applies latin variant |
| `src/usecase.rs` | Update fork sandbox call to use `prescan()` |
| `src/theme.rs` | Add latin `ThemeEntry`s, add `resolve_latin_variant()`, `LATIN_VARIANTS` |
| `themes/catppuccin-latin.typ` | New: Inter-based dark theme |
| `themes/catppuccin-latte-latin.typ` | New: Inter-based light theme |
| `Cargo.toml` | Rename feature `embed-noto-fonts` → `embed-fonts` |
| `build.rs` | Update feature gate env var |
| `tests/integration.rs` | Update `extract_image_paths` calls |

## Files NOT changed

- `src/config.rs` — no new config fields (auto-detection, no user setting needed)
- `src/pipeline/world.rs` / `FontCache` — font loading unchanged
- `src/viewer/mod.rs` — `resolve_theme_name()` call unchanged (Phase 1 only)
- CLI arguments — no additions

## Open questions

- Code block font in latin mode: drop Noto Sans JP fallback entirely, or keep as
  distant fallback? (Decide based on output quality inspection)
- Italic rendering: verify Typst auto-selects Inter-Italic for `#emph`. If not,
  add explicit `#show emph: set text(style: "italic")`
