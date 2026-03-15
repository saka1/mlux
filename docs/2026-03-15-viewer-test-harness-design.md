# Viewer Test Harness Design

## Problem

The viewer's yank, highlight, and display logic is hard to debug because:

1. The viewer requires a Kitty-compatible terminal to run
2. Unit tests cover pure logic (mode handlers, scroll math) but cannot exercise end-to-end scenarios
3. When bugs appear, it's difficult to determine whether the logic output or the rendering is wrong
4. Coding agents cannot autonomously reproduce and debug viewer issues

## Goal

Enable scenario-based testing: build a document from markdown, feed a key sequence, and assert on the resulting state and rendering intent — all without a terminal.

## Architecture

Three changes, ordered by dependency:

### 1. RenderOp extraction from `apply()`

`Viewport::apply()` currently mixes state mutation with terminal I/O. Separate them.

**New enum** in `effect.rs`:

```rust
pub(super) enum RenderOp {
    DrawStatusBar,
    DrawModeScreen,
    ClearAndReset,
    CopyToClipboard(String),
    OpenExternal(String),
    DeletePlacements,
    DeleteOverlayPlacements,
}
```

Coarse granularity is intentional. `DrawStatusBar` carries no data because its content is deterministic from `Viewport` state — tests assert on state directly. Only `CopyToClipboard` carries a payload (for yank testing).

**Signature change**:

```rust
// Before
fn apply(&mut self, effect: Effect, ctx: &ViewContext) -> Result<Option<ExitReason>>

// After
fn apply(
    &mut self,
    effect: Effect,
    ctx: &ViewContext,
    ops: &mut Vec<RenderOp>,
) -> Result<Option<ExitReason>>
```

Each I/O call in `apply()` becomes a `ops.push(RenderOp::...)` instead. The event loop drains `ops` and passes them to a new `execute_render_ops()` function that performs the actual terminal I/O.

**Mapping** (current I/O → RenderOp):

| Current call in `apply()` | RenderOp |
|---|---|
| `terminal::draw_status_bar(...)` | `DrawStatusBar` |
| `mode_search::draw_search_screen(...)` | `DrawModeScreen` |
| `terminal::draw_command_bar(...)` | `DrawModeScreen` |
| `mode_url::draw_url_screen(...)` | `DrawModeScreen` |
| `mode_toc::draw_toc_screen(...)` | `DrawModeScreen` |
| `terminal::send_osc52(text)` | `CopyToClipboard(text)` |
| `open::that_in_background(url)` | `OpenExternal(url)` |
| `terminal::clear_screen()` + `delete_all_images()` | `ClearAndReset` |
| `self.tiles.delete_placements()` | `DeletePlacements` |
| `self.tiles.clear_overlays()` (deletes highlight placements) | `DeleteOverlayPlacements` |

**`RenderOp::DeleteOverlayPlacements`**: `tiles.clear_overlays()` does two things: (1) clears the internal `overlay_rects` HashMap (state mutation), and (2) sends KGP delete commands for highlight images (I/O). Split these: keep the state mutation in `apply()`, push `DeleteOverlayPlacements` for the I/O. This affects `Effect::SetLastSearch` and `Effect::InvalidateOverlays`.

**`SetMode(ViewerMode::Normal)`** decomposition:

```rust
// State mutation (stays in apply):
self.tiles.clear_all();   // clears internal HashMap, no I/O
self.dirty = true;
self.mode = m;

// I/O (becomes RenderOps):
ops.push(RenderOp::ClearAndReset);  // clear_screen + delete_all_images
```

**`EnterUrlPickerAll`** decomposition:

```rust
// Pseudo-code for the refactored arm:
let entries = mode_url::collect_all_url_entries(ctx.doc);
if entries.is_empty() {
    self.flash = Some("No URLs in document".into());
    if !matches!(self.mode, ViewerMode::Normal) {
        self.tiles.map.clear();          // state mutation
        self.mode = ViewerMode::Normal;
        self.dirty = true;
        ops.push(RenderOp::ClearAndReset);
    } else {
        ops.push(RenderOp::DrawStatusBar);
    }
} else {
    ops.push(RenderOp::DeletePlacements);
    ops.push(RenderOp::ClearAndReset);
    let up = UrlPickerState::new(entries);
    self.mode = ViewerMode::UrlPicker(up);
    ops.push(RenderOp::DrawModeScreen);
}
```

**`GoBack`** with empty stack: set `self.flash`, push `DrawStatusBar`.

### 2. Synchronous highlight rect computation

Currently, highlight rects are computed via worker thread (`WorkerRequest::FindRects` → fork process → `TiledDocument::find_tile_highlight_rects()`). The underlying function is already pure:

```rust
// highlight.rs — pure function, no I/O
pub fn find_highlight_rects(
    frame: &Frame,
    spec: &HighlightSpec,
    ppi: f32,
    source: &Source,
) -> Vec<HighlightRect>
```

For the test harness, expose a method on `TiledDocument` (already exists as `find_tile_highlight_rects`) and make the document available to the harness. No new code needed — just access.

### 3. TestHarness

A `#[cfg(test)]` struct that wires together document building, event processing, and state inspection.

```rust
// src/viewer/test_harness.rs (new file, cfg(test) only)

pub struct TestHarness {
    pub viewport: Viewport,
    layout: Layout,
    meta: DocumentMeta,
    doc: TiledDocument,       // retained for find_tile_highlight_rects()
    markdown: String,
    acc: InputAccumulator,
    input_source: InputSource,
    filename: String,
    render_ops: Vec<RenderOp>,
}
// Note: content_index and content_offset come from DocumentMeta
// (meta.content_index, meta.content_offset). No separate fields needed.

impl TestHarness {
    /// Build a harness from markdown and terminal dimensions.
    ///
    /// Uses `build_tiled_document()` internally. Pixel dimensions are
    /// derived from (cols, rows) assuming a fixed cell size (e.g. 10x20).
    pub fn new(md: &str, cols: u16, rows: u16) -> Self;

    /// Process a single key event through the full pipeline:
    /// input mapping → mode handler → effects → apply().
    /// Returns the RenderOps produced.
    pub fn feed_key(&mut self, key: KeyEvent) -> Vec<RenderOp>;

    /// Parse a string into key events and feed them sequentially.
    /// Uppercase letters (e.g. 'G', 'N') are sent as KeyCode::Char('G')
    /// with KeyModifiers::SHIFT, matching crossterm's convention.
    /// Special sequences: \n = Enter, \x1b = Escape.
    /// For modifier keys (Alt, Ctrl), use feed_key() directly.
    pub fn feed_keys(&mut self, keys: &str);

    // --- State inspection ---
    pub fn scroll_y(&self) -> u32;
    pub fn is_dirty(&self) -> bool;
    pub fn flash(&self) -> Option<&str>;

    /// Extract the last CopyToClipboard text from render_ops.
    pub fn last_yanked(&self) -> Option<&str>;

    /// Compute visible tiles for current scroll position.
    pub fn visible_tiles(&self) -> VisibleTiles;

    /// Compute highlight rects synchronously for a given tile.
    /// Uses last_search to build HighlightSpec, then calls
    /// find_tile_highlight_rects() directly.
    pub fn highlight_rects(&self, tile_idx: usize) -> Vec<HighlightRect>;

    /// All RenderOps accumulated since last feed_key/feed_keys call.
    pub fn render_ops(&self) -> &[RenderOp];
}
```

**Document building**: reuses the same pipeline as integration tests (`FontCache`, `MluxWorld`, `compile_document`, `build_tiled_document`). Uses default theme, fixed `width_pt=400.0`, `ppi=144.0` to match existing test conventions. The harness retains the full `TiledDocument` (not just `DocumentMeta`) because `find_tile_highlight_rects()` needs access to `Frame`s and the `Source`.

**Mode handler context**: `feed_key()` computes `max_scroll` from `meta.max_scroll(vp.scroll.vp_h)` and constructs `DocumentQuery` from `meta.content_index`, `meta.content_offset`, and `markdown`, mirroring the event loop in `mod.rs:420-426`.

**Layout construction**: `layout::compute_layout()` needs pixel dimensions. The harness uses synthetic values (e.g., cell_w=10, cell_h=20, pixel_width=cols*10, pixel_height=rows*20) to create a deterministic `Layout`.

**No tile PNG rendering**: the harness never calls `render_frame_to_png()`. Tile PNG data is not needed for state testing. The `TiledDocumentCache` stays empty — this is fine because the harness never calls `redraw()`.

## What this does NOT change

- **`redraw()` (tiles.rs)**: not modified. Its inputs (visible_tiles, overlay_rects, scroll state) are already inspectable from the harness.
- **`terminal.rs`**: not modified. The `execute_render_ops()` function is a new thin wrapper that calls existing terminal functions.
- **Mode handlers**: not modified. They already return `Vec<Effect>` — pure logic.
- **`Effect` enum**: preserved as-is. The Effect variants (`RedrawStatusBar`, `RedrawSearch`, etc.) continue to exist; `apply()` translates them to `RenderOp`s. This keeps mode handler code unchanged.
- **Worker thread / fork_render**: not modified. The harness bypasses them by calling `find_tile_highlight_rects()` directly.

## Example tests

### Yank produces correct text

```rust
#[test]
fn yank_heading() {
    let mut h = TestHarness::new("# Hello\n\nWorld\n", 80, 24);
    // '1y' yanks visual line 1 (the heading)
    h.feed_keys("1y");
    assert_eq!(h.last_yanked(), Some("# Hello"));
}
```

### Search sets highlight rects

```rust
#[test]
fn search_creates_highlight_rects() {
    let mut h = TestHarness::new("# Title\n\nfoo bar foo\n", 80, 24);
    h.feed_keys("/foo\n");  // search and confirm

    // Should be back in normal mode with search active
    assert!(matches!(h.viewport.mode, ViewerMode::Normal));
    assert!(h.viewport.last_search.is_some());

    // Highlight rects should cover both "foo" occurrences
    let rects = h.highlight_rects(0);
    assert_eq!(rects.len(), 2);
    assert!(rects[0].w_px > 0);
}
```

### Scroll position after jump

```rust
#[test]
fn jump_to_line_scrolls() {
    let long_md = "# Title\n\n".to_owned() + &"line\n".repeat(100);
    let mut h = TestHarness::new(&long_md, 80, 24);
    h.feed_keys("50G");  // jump to line 50
    assert!(h.scroll_y() > 0);
    assert!(h.is_dirty());
}
```

### RenderOp sequence verification

```rust
#[test]
fn entering_search_mode_emits_draw() {
    let mut h = TestHarness::new("# Hello\n", 80, 24);
    let ops = h.feed_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(ops.iter().any(|op| matches!(op, RenderOp::DrawModeScreen)));
}
```

## Testing scope after implementation

| Problem area | What the harness enables | Remaining blind spot |
|---|---|---|
| **Yank** | Assert exact yanked text for any key sequence + document | None (full coverage) |
| **Highlight** | Assert rect count, positions, active flag for any search | KGP placement coordinate math (pure, testable separately) |
| **Display glitches** | Assert scroll position, visible tiles, dirty flag | Actual pixel rendering (terminal-only) |
| **Mode transitions** | Assert mode + state after any key sequence | Already well-tested, this adds scenario coverage |

## File changes summary

| File | Change |
|---|---|
| `src/viewer/effect.rs` | Add `RenderOp` enum, change `apply()` signature |
| `src/viewer/mod.rs` | Add `execute_render_ops()`, update apply call sites |
| `src/viewer/test_harness.rs` | New file (`#[cfg(test)]` only) |
| `tests/viewer_harness.rs` or inline | Test cases using the harness |
