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

/// Why the inner event loop exited back to the outer rebuild loop.
#[derive(Debug, Clone)]
pub(super) enum ExitReason {
    Quit,
    Resize { new_cols: u16, new_rows: u16 },
    Reload,
    ConfigReload,
    Navigate { path: std::path::PathBuf },
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

/// Side-effect descriptors produced by mode handlers.
///
/// Handlers return `Vec<Effect>` which the apply loop in `run()` executes.
/// This separates "what to do" (handler) from "how to do it" (apply loop).
pub(super) enum Effect {
    ScrollTo(u32),
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
    SetLastSearch(LastSearch),
    DeletePlacements,
    /// Clear overlay rects cache so they're recomputed with new active_ranges.
    InvalidateOverlays,
    EnterUrlPickerAll,
    EnterLog,
    GoBack,
    Exit(ExitReason),
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
    vp: &Viewport,
    ctx: &ViewContext,
) -> anyhow::Result<Option<ExitReason>> {
    for op in ops {
        match op {
            RenderOp::DrawStatusBar => {
                if let ViewerMode::InlineSearch(is) = &vp.mode {
                    terminal::draw_inline_search_bar(
                        ctx.layout,
                        &is.query,
                        is.current_idx,
                        is.matches.len(),
                    )?;
                } else {
                    terminal::draw_status_bar(
                        ctx.layout,
                        &vp.scroll,
                        ctx.filename,
                        ctx.acc_value,
                        vp.flash.as_deref(),
                    )?;
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
