# Viewport Refactor: Conceptual Separation of State, Effects, and Session

## Motivation

`effect.rs` currently holds three distinct responsibilities: effect type definitions
(Effect, RenderOp, ExitReason), screen state and its transition function (Viewport,
ViewContext, apply), and persistent session state (Session). Viewport is "the current
screen state," not an effect concept — it belongs in its own module.

## Design Decisions

1. **Viewport is a value-typed state machine.** `apply` takes ownership and returns
   `(Self, Vec<RenderOp>)` — a pure transition function with no `Result`, no `&mut self`.

2. **ExitReason folds into RenderOp.** `RenderOp::Exit(ExitReason)` replaces the
   `Option<ExitReason>` return value. The apply signature becomes a clean 2-tuple.

3. **ViewContext stays as a parameter bundle.** It has no independent identity — it's
   a view into Session + doc for apply's convenience. Co-located with Viewport.

4. **Three-file split by responsibility:**
   - `effect.rs` — effect vocabulary (Effect, RenderOp, ExitReason, ViewerMode) + execute_render_ops
   - `viewport.rs` — screen state and transition (Viewport, ViewContext, apply, link helpers)
   - `session.rs` — persistent state across rebuilds (Session, JumpEntry, handle_exit)

## File Structure

### `effect.rs` — What to do

```rust
pub(super) enum Effect { ScrollTo(u32), MarkDirty, Flash(String), ... }

pub(super) enum RenderOp {
    DrawStatusBar,
    DrawModeScreen,
    ClearScreen,
    DeleteAllImages,
    CopyToClipboard(String),
    OpenExternal(String),
    DeletePlacements,
    DeleteOverlayPlacements,
    Exit(ExitReason),          // NEW: was a separate return value
}

pub(super) enum ExitReason { Quit, Resize { new_cols: u16, new_rows: u16 }, Reload, ... }
pub(super) enum ViewerMode { Normal, Search(SearchState), Command(CommandState), ... }

/// Executes terminal I/O. Takes ownership of ops to move ExitReason out
/// without requiring Clone. Short-circuits on first Exit encountered.
pub(super) fn execute_render_ops(
    ops: Vec<RenderOp>,
    vp: &Viewport,
    ctx: &ViewContext,
) -> anyhow::Result<Option<ExitReason>>
```

### `viewport.rs` — Screen state and transitions

```rust
pub(super) struct Viewport {
    pub mode: ViewerMode,
    pub scroll: ScrollState,
    pub display: DisplayState,
    pub flash: Option<String>,
    pub dirty: bool,
    pub last_search: Option<LastSearch>,
}

/// Read-only environment for apply — a view into Session + doc.
pub(super) struct ViewContext<'a> {
    pub layout: &'a Layout,
    pub acc_value: Option<u32>,
    pub input: &'a InputSource,
    pub filename: &'a str,
    pub jump_stack: &'a [JumpEntry],
    pub doc: &'a DocumentQuery<'a>,
    pub log_buffer: &'a crate::log::LogBuffer,
}

impl Viewport {
    /// Pure state transition. No Result, no &mut self.
    pub(super) fn apply(mut self, effect: Effect, ctx: &ViewContext) -> (Self, Vec<RenderOp>) {
        let mut ops = Vec::new();
        match effect {
            Effect::ScrollTo(y) => { /* snap + set dirty */ }
            Effect::Exit(reason) => { ops.push(RenderOp::Exit(reason)); }
            Effect::OpenUrl(url) => {
                if is_local_markdown_link(&url) && ... {
                    ops.push(RenderOp::Exit(ExitReason::Navigate { path }));
                } else {
                    ops.push(RenderOp::OpenExternal(url));
                }
            }
            // ... all other variants unchanged in logic
        }
        (self, ops)
    }
}

fn is_local_markdown_link(url: &str) -> bool { ... }
fn resolve_link_path(url: &str, current_file: &Path) -> Option<PathBuf> { ... }
```

### `session.rs` — Persistent state across rebuilds

```rust
pub(super) struct JumpEntry { pub path: PathBuf, pub y_offset: u32 }

pub(super) struct Session {
    pub layout: Layout,
    pub input: InputSource,
    pub filename: String,
    pub watcher: Option<FileWatcher>,
    pub jump_stack: Vec<JumpEntry>,
    pub scroll_carry: u32,
    pub pending_flash: Option<String>,
    pub watch: bool,
    pub log_buffer: crate::log::LogBuffer,
}

impl Session {
    pub(super) fn update_layout_for_resize(...) -> Result<()> { ... }
    pub(super) fn handle_exit(...) -> Result<bool> { ... }
}
```

## Caller Changes

### `mod.rs` — Inner loop

```rust
// Before
for effect in effects {
    let mut render_ops = Vec::new();
    if let Some(reason) = vp.apply(effect, &ctx, &mut render_ops)? {
        execute_render_ops(&render_ops, &vp, &ctx)?;
        return Ok(reason);
    }
    execute_render_ops(&render_ops, &vp, &ctx)?;
}

// After
for effect in effects {
    let (new_vp, render_ops) = vp.apply(effect, &ctx);
    vp = new_vp;
    if let Some(reason) = effect::execute_render_ops(render_ops, &vp, &ctx)? {
        return Ok(reason);
    }
}
```

### `test_harness.rs`

```rust
// Before
for effect in effects {
    let mut effect_ops = Vec::new();
    match self.viewport.apply(effect, &ctx, &mut effect_ops) {
        Ok(Some(_exit)) => { ops.extend(effect_ops); break; }
        Ok(None) => ops.extend(effect_ops),
        Err(e) => panic!("apply failed: {e}"),
    }
}

// After — std::mem::replace needed because self.viewport is behind &mut self
for effect in effects {
    let vp = std::mem::take(&mut self.viewport);
    let (new_vp, effect_ops) = vp.apply(effect, &ctx);
    self.viewport = new_vp;
    let has_exit = effect_ops.iter().any(|op| matches!(op, RenderOp::Exit(_)));
    ops.extend(effect_ops);
    if has_exit { break; }
}
// Note: Viewport must impl Default (or provide a dummy) for std::mem::take.
// Alternative: wrap in Option<Viewport> and use .take().unwrap().
```

## Impact

- **Logic changes: none.** All effect handling logic is preserved exactly.
- **Files modified:** effect.rs, mod.rs, test_harness.rs
- **Files created:** viewport.rs, session.rs
- **Files unaffected:** mode_normal.rs, mode_search.rs, mode_command.rs, mode_toc.rs,
  mode_url.rs, mode_log.rs, display_state.rs, keymap.rs, terminal.rs, layout.rs, query.rs

### Migration details

- **Module declarations:** `mod.rs` adds `mod viewport;` and `mod session;`
- **Import path changes:** mod.rs and test_harness.rs update imports for Viewport/ViewContext
  (from `effect::` to `viewport::`) and Session (from `effect::` to `session::`)
- **`execute_render_ops` takes `Vec<RenderOp>` by value** (not `&[RenderOp]`) so ExitReason
  can be moved out without Clone. Short-circuits on first `RenderOp::Exit`.
- **test_harness.rs ownership:** `self.viewport` is behind `&mut self`, so `std::mem::take`
  (with Default impl) or `Option<Viewport>` wrapping is needed to move it into `apply`.
- **Tests move with helpers:** `test_is_local_markdown_link` and `test_resolve_link_path`
  move from effect.rs to viewport.rs alongside the link helper functions.
- **viewport.rs imports from mode_*:** needs `use super::mode_search::*`, `mode_log::*`,
  `mode_toc::*`, `mode_url::*` for types used in apply (SearchState, LogState, etc.).
- **Circular sibling imports:** effect.rs imports Viewport from viewport.rs (for
  execute_render_ops signature), viewport.rs imports Effect/RenderOp from effect.rs.
  This is fine — Rust allows sibling module cross-imports.
