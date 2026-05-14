//! Effect vocabulary, render operations, and terminal I/O execution.
//!
//! Defines the "what to do" types (`Effect`, `RenderOp`, `ExitReason`, `ViewerMode`)
//! and the `execute_render_ops` function that performs terminal I/O.
//!
//! Screen state and transitions live in `viewport.rs`.
//! Persistent session state lives in `session.rs`.

use super::mode_command::CommandState;
use super::mode_grep::{self, GrepState, LastSearch};
use super::mode_inline_search::InlineSearchState;
use super::mode_log::{self, LogState};
use super::mode_toc::{self, TocState};
use super::mode_url::{self, UrlPickerState};
use super::terminal;
use super::viewport::{ViewContext, Viewport};
use std::io::Write;

/// Why the inner event loop exited back to the outer rebuild loop.
#[derive(Debug, Clone)]
pub(super) enum ExitReason {
    Quit,
    Resize {
        new_cols: u16,
        new_rows: u16,
    },
    Reload,
    /// Scale (zoom) factor changed; outer loop must rebuild with the new value.
    /// Carries `old` so the scroll position can be scaled to keep the same
    /// point in the document anchored after the rebuild.
    SetScale {
        old: f64,
        new: f64,
        /// Status-bar flash text to surface after the rebuild (e.g. "zoom: 125%").
        /// Carried via `Session::pending_flash` so the indicator survives the
        /// document rebuild that any scale change triggers.
        flash: Option<String>,
    },
    Navigate {
        path: std::path::PathBuf,
    },
    GoBack,
}

/// Viewer mode: normal (tile display), search (picker UI), command (`:` prompt), URL picker, or log viewer.
pub(super) enum ViewerMode {
    Normal,
    Grep(GrepState),
    InlineSearch(InlineSearchState),
    Command(CommandState),
    UrlPicker(UrlPickerState),
    Toc(TocState),
    Log(LogState),
}

/// How to restore the screen when returning to Normal mode.
/// The exiting mode chooses the appropriate variant.
pub(super) enum ScreenRestore {
    /// Full-screen mode occupied the entire screen: ClearScreen + DeleteAllImages + clear_all.
    FullRefresh,
    /// Only the status bar was modified: redraw status bar only.
    StatusBarRefresh,
}

/// Side-effect descriptors produced by mode handlers.
///
/// Handlers return `Vec<Effect>` which the apply loop in `run()` executes.
/// This separates "what to do" (handler) from "how to do it" (apply loop).
pub(super) enum Effect {
    /// Incremental scroll (j, k, Ctrl-D, Ctrl-U).  The signed
    /// `delta_px` is pushed to the input history with `timestamp = now`;
    /// the animator derives position as a closed-form function of
    /// `(anchor, history, now)`.  `direction` is carried explicitly
    /// because it can differ from `delta_px.signum()` only in the
    /// degenerate clamp-to-edge case (delta = 0); keeping it explicit
    /// matches what scroll_policy expects.
    ScrollImpulse {
        delta_px: i32,
        direction: super::input_history::ScrollDirection,
    },
    /// Absolute scroll jump (gg, G, Ngg, TOC, search).  Drains the
    /// in-flight history (cancelling any residual momentum), pins the
    /// anchor to the current sub-pixel position, and re-pushes a
    /// single impulse for `(target - current)` so velocity-based
    /// animators glide smoothly to the new anchor (matches legacy
    /// `set_landing` semantics).
    ScrollAnchor(u32),
    MarkDirty,
    Flash(String),
    RedrawStatusBar,
    RedrawGrep,
    RedrawCommandBar,
    RedrawUrlPicker,
    RedrawToc,
    RedrawInlineSearch,
    RedrawLog,
    Yank(String),
    OpenExternalUrl(String),
    SetMode(ViewerMode),
    /// Return to Normal mode with the specified screen restoration.
    ExitToNormal(ScreenRestore),
    SetLastSearch(LastSearch),
    DeletePlacements,
    /// Clear overlay rects cache so they're recomputed with new active_ranges.
    InvalidateOverlays,
    EnterUrlPickerAll,
    EnterLog,
    GoBack,
    Exit(ExitReason),
    HideHighlights,
    ShowHighlights,
    ToggleWatch,
    /// Accumulate signed zoom delta (in preset steps) into the upper loop.
    /// Coalesced into a single `Effect::Exit(SetScale)` per frame budget so
    /// burst Ctrl+wheel input doesn't trigger one full rebuild per notch.
    AccumulateZoom(i32),
}

/// Terminal I/O operations separated from state mutation.
///
/// `Viewport::apply()` pushes these instead of performing I/O directly.
/// The event loop drains them via `execute_render_ops()`.
#[derive(Debug, Clone)]
pub(super) enum RenderOp {
    DrawStatusBar,
    DrawModeScreen,
    ClearScreen,
    DeleteAllImages,
    CopyToClipboard(String),
    OpenExternalUrl(String),
    DeletePlacements,
    DeleteOverlayPlacements,
    Exit(ExitReason),
}

/// Execute terminal I/O operations deferred from apply().
///
/// Takes ownership of ops so `ExitReason` can be moved out without Clone.
/// Short-circuits on the first `RenderOp::Exit` encountered.
pub(super) fn execute_render_ops(
    ops: Vec<RenderOp>,
    vp: &mut Viewport,
    ctx: &ViewContext,
) -> anyhow::Result<Option<ExitReason>> {
    for op in ops {
        match op {
            RenderOp::DrawStatusBar => {
                if let ViewerMode::InlineSearch(is) = &vp.mode {
                    use super::mode_grep::SearchDirection;
                    let prompt = match is.direction {
                        SearchDirection::Forward => '/',
                        SearchDirection::Backward => '?',
                    };
                    terminal::draw_inline_search_bar(ctx.layout, &is.query, prompt)?;
                } else {
                    let mut out = std::io::stdout();
                    terminal::draw_status_bar(
                        &mut out,
                        ctx.layout,
                        &vp.scroll,
                        ctx.filename,
                        ctx.acc_value,
                        vp.flash.as_deref(),
                    )?;
                    out.flush()?;
                }
            }
            RenderOp::DrawModeScreen => match &vp.mode {
                ViewerMode::Grep(gs) => {
                    mode_grep::draw_search_screen(
                        ctx.layout,
                        &gs.query,
                        &gs.matches,
                        gs.selected,
                        gs.scroll_offset,
                        gs.pattern_valid,
                    )?;
                }
                ViewerMode::Command(cs) => {
                    terminal::draw_command_bar(ctx.layout, &cs.input)?;
                }
                ViewerMode::UrlPicker(up) => {
                    mode_url::draw_url_screen(ctx.layout, up)?;
                }
                ViewerMode::Toc(ts) => {
                    mode_toc::draw_toc_screen(ctx.layout, ts)?;
                }
                ViewerMode::Log(ls) => {
                    mode_log::draw_log_screen(ctx.layout, ls)?;
                }
                ViewerMode::InlineSearch(_) => {
                    // InlineSearch doesn't use DrawModeScreen — it draws via status bar
                }
                ViewerMode::Normal => {}
            },
            RenderOp::ClearScreen => {
                terminal::clear_screen()?;
            }
            RenderOp::DeleteAllImages => {
                terminal::delete_all_images()?;
            }
            RenderOp::CopyToClipboard(text) => {
                let _ = terminal::send_osc52(&text);
            }
            RenderOp::OpenExternalUrl(url) => {
                let _ = open::that_in_background(&url);
            }
            RenderOp::DeletePlacements => {
                vp.display.delete_placements()?;
            }
            RenderOp::DeleteOverlayPlacements => {
                vp.display.delete_overlay_placements()?;
            }
            RenderOp::Exit(reason) => {
                return Ok(Some(reason));
            }
        }
    }
    Ok(None)
}
