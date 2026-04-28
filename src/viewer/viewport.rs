//! Screen state and transition logic for the viewer.
//!
//! `Viewport` is the user's interactive view into a document build — a value-typed
//! state machine whose `apply` method is a pure transition function returning
//! `(Self, Vec<RenderOp>)`.

use super::display_state::DisplayState;
use super::effect::{Effect, ExitReason, RenderOp, ScreenRestore, ViewerMode};
use super::layout::ScrollState;
use super::mode_grep::LastSearch;
use super::mode_log::LogState;
use super::mode_url::{self, UrlPickerState};
use super::query::DocumentQuery;
use super::session::JumpEntry;

/// The user's interactive view into a document build.
///
/// Contains all mutable state for the inner event loop: interaction mode,
/// scroll position, tile cache, and transient UI state. Created fresh
/// for each document build; destroyed when the build is replaced.
pub(super) struct Viewport {
    pub mode: ViewerMode,
    pub scroll: ScrollState,
    pub display: DisplayState,
    pub flash: Option<String>,
    pub dirty: bool,
    pub last_search: Option<LastSearch>,
    pub highlights_visible: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            mode: ViewerMode::Normal,
            scroll: ScrollState::new(0, 0, 0, 0, crate::config::ScrollAnimation::default()),
            display: DisplayState::new(0),
            flash: None,
            dirty: false,
            last_search: None,
            highlights_visible: true,
        }
    }
}

/// Read-only environment for effect application.
///
/// Bundles references to layout, document content, and navigation state
/// that `Viewport::apply` needs but must not modify.
pub(super) struct ViewContext<'a> {
    pub layout: &'a super::layout::Layout,
    pub acc_value: Option<u32>,
    pub filename: &'a str,
    pub jump_stack: &'a [JumpEntry],
    pub doc: &'a DocumentQuery<'a>,
    pub log_buffer: &'a crate::log::LogBuffer,
}

impl Viewport {
    /// Pure state transition. Returns updated viewport and render ops.
    ///
    /// No Result, no &mut self — takes ownership and returns new state.
    /// Exit conditions are signaled via `RenderOp::Exit`.
    pub(super) fn apply(mut self, effect: Effect, ctx: &ViewContext) -> (Self, Vec<RenderOp>) {
        let mut ops = Vec::new();
        match effect {
            Effect::ScrollTo(y) => {
                // Absolute jump (gg/G/search/TOC). Position-chase animators
                // chase the new target; Kinetic overrides velocity via
                // set_landing so a flick already in progress is cancelled
                // and the glide lands at y (iOS scroll-to-top semantics).
                // `restart_ramp` is true only when the animation was settled so that
                // continuous keypresses (j held down) don't re-trigger ease-in every frame.
                let restart_ramp = !self
                    .scroll
                    .animator
                    .is_animating(self.scroll.target_y as f64);
                self.scroll.target_y = y;
                self.scroll.animator.set_target(restart_ramp);
                self.scroll.animator.set_landing(y as f64);
                self.dirty = true;
            }
            Effect::ScrollBy { target, impulse_px } => {
                // Incremental scroll (j/k/Ctrl-D/Ctrl-U). Update target for
                // position-chase animators, apply impulse for Kinetic so
                // rapid keypresses stack as momentum.
                let restart_ramp = !self
                    .scroll
                    .animator
                    .is_animating(self.scroll.target_y as f64);
                self.scroll.target_y = target;
                self.scroll.animator.set_target(restart_ramp);
                self.scroll.animator.add_impulse(impulse_px as f64);
                self.dirty = true;
            }
            Effect::MarkDirty => {
                self.dirty = true;
            }
            Effect::Flash(msg) => {
                self.flash = Some(msg);
            }
            Effect::RedrawStatusBar => {
                ops.push(RenderOp::DrawStatusBar);
            }
            Effect::RedrawGrep => {
                if matches!(self.mode, ViewerMode::Grep(_)) {
                    ops.push(RenderOp::DrawModeScreen);
                }
            }
            Effect::RedrawCommandBar => {
                if matches!(self.mode, ViewerMode::Command(_)) {
                    ops.push(RenderOp::DrawModeScreen);
                }
            }
            Effect::RedrawUrlPicker => {
                if matches!(self.mode, ViewerMode::UrlPicker(_)) {
                    ops.push(RenderOp::DrawModeScreen);
                }
            }
            Effect::RedrawToc => {
                if matches!(self.mode, ViewerMode::Toc(_)) {
                    ops.push(RenderOp::DrawModeScreen);
                }
            }
            Effect::RedrawInlineSearch => {
                if matches!(self.mode, ViewerMode::InlineSearch(_)) {
                    ops.push(RenderOp::DrawStatusBar);
                }
            }
            Effect::RedrawLog => {
                if matches!(self.mode, ViewerMode::Log(_)) {
                    ops.push(RenderOp::DrawModeScreen);
                }
            }
            Effect::Yank(text) => {
                ops.push(RenderOp::CopyToClipboard(text));
            }
            Effect::OpenExternalUrl(url) => {
                ops.push(RenderOp::OpenExternalUrl(url));
            }
            Effect::SetMode(m) => {
                match &m {
                    ViewerMode::InlineSearch(_) => {
                        ops.push(RenderOp::DrawStatusBar);
                    }
                    ViewerMode::Normal => {
                        unreachable!("use ExitToNormal to return to Normal mode");
                    }
                    _ => {
                        ops.push(RenderOp::DrawModeScreen);
                    }
                }
                self.mode = m;
            }
            Effect::ExitToNormal(restore) => {
                match restore {
                    ScreenRestore::FullRefresh => {
                        ops.push(RenderOp::ClearScreen);
                        ops.push(RenderOp::DeleteAllImages);
                        self.display.clear_all();
                    }
                    ScreenRestore::StatusBarRefresh => {
                        ops.push(RenderOp::DrawStatusBar);
                    }
                }
                self.mode = ViewerMode::Normal;
                self.dirty = true;
            }
            Effect::SetLastSearch(ls) => {
                self.last_search = Some(ls);
                self.highlights_visible = true;
                self.display.clear_overlay_state();
                ops.push(RenderOp::DeleteOverlayPlacements);
                self.dirty = true;
            }
            Effect::HideHighlights => {
                self.highlights_visible = false;
                self.display.clear_overlay_state();
                ops.push(RenderOp::DeleteOverlayPlacements);
                self.dirty = true;
            }
            Effect::ShowHighlights => {
                self.highlights_visible = true;
                self.display.clear_overlay_state();
                ops.push(RenderOp::DeleteOverlayPlacements);
                self.dirty = true;
            }
            Effect::DeletePlacements => {
                ops.push(RenderOp::DeletePlacements);
            }
            Effect::InvalidateOverlays => {
                self.display.clear_overlay_state();
                ops.push(RenderOp::DeleteOverlayPlacements);
                self.dirty = true;
            }
            Effect::EnterUrlPickerAll => {
                let entries = mode_url::collect_all_url_entries(ctx.doc);
                if entries.is_empty() {
                    self.flash = Some("No URLs in document".into());
                    if !matches!(self.mode, ViewerMode::Normal) {
                        ops.push(RenderOp::ClearScreen);
                        ops.push(RenderOp::DeleteAllImages);
                        self.display.clear_all();
                        self.mode = ViewerMode::Normal;
                        self.dirty = true;
                    } else {
                        ops.push(RenderOp::DrawStatusBar);
                    }
                } else {
                    ops.push(RenderOp::DeletePlacements);
                    ops.push(RenderOp::ClearScreen);
                    // NO DeleteAllImages — original code doesn't call it here
                    let up = UrlPickerState::new(entries);
                    self.mode = ViewerMode::UrlPicker(up);
                    ops.push(RenderOp::DrawModeScreen);
                }
            }
            Effect::EnterLog => {
                let ls = LogState::new(ctx.log_buffer);
                self.mode = ViewerMode::Log(ls);
                ops.push(RenderOp::DrawModeScreen);
            }
            Effect::GoBack => {
                if ctx.jump_stack.is_empty() {
                    self.flash = Some("No previous file".into());
                    ops.push(RenderOp::DrawStatusBar);
                } else {
                    ops.push(RenderOp::Exit(ExitReason::GoBack));
                }
            }
            Effect::ToggleWatch => {
                // Handled in mod.rs effect loop (needs Session access)
            }
            Effect::Exit(reason) => {
                ops.push(RenderOp::Exit(reason));
            }
        }
        (self, ops)
    }
}
