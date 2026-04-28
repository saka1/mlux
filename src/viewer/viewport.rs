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
    /// Accumulated Ctrl+wheel zoom steps awaiting flush by the outer loop.
    /// Positive = zoom in, negative = zoom out. Drained at frame-budget
    /// boundaries to avoid one full document rebuild per wheel notch.
    pub pending_zoom_delta: i32,
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
            pending_zoom_delta: 0,
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
    /// Accumulate a Ctrl+wheel zoom step and request a frame-budget-bounded
    /// flush. Marking dirty switches the next `event::poll` timeout from the
    /// long `watch_interval` to `frame_budget`, which is where the outer loop
    /// drains `pending_zoom_delta` into a single SetScale rebuild.
    pub(super) fn accumulate_zoom_delta(&mut self, delta: i32) {
        self.pending_zoom_delta = self.pending_zoom_delta.saturating_add(delta);
        self.dirty = true;
    }

    /// Pure state transition. Returns updated viewport and render ops.
    ///
    /// No Result, no &mut self — takes ownership and returns new state.
    /// Exit conditions are signaled via `RenderOp::Exit`.
    pub(super) fn apply(mut self, effect: Effect, ctx: &ViewContext) -> (Self, Vec<RenderOp>) {
        let mut ops = Vec::new();
        match effect {
            Effect::ScrollImpulse {
                delta_px,
                direction,
            } => {
                let now = std::time::Instant::now();
                // restart_ramp captures whether the animation was settled
                // BEFORE this impulse — used by ExpDecay's ease-in.
                let was_settled = !self.scroll.animator.is_animating(
                    self.scroll.anchor,
                    &self.scroll.input_history,
                    now,
                );
                // Push to history; convolve evicted entries into the
                // permanent anchor using their actual contribution at
                // this instant (exact for Kinetic, full δ for ExpDecay).
                let evicted = self.scroll.input_history.record(direction, delta_px);
                for r in &evicted {
                    self.scroll.anchor += self.scroll.animator.eviction_contribution(r, now);
                }
                self.scroll.animator.restart_ease_in_if_settled(was_settled);
                self.dirty = true;
            }
            Effect::ScrollAnchor(y) => {
                // iOS scroll-to-top semantics: cancel residual momentum,
                // pin the anchor to the current sub-pixel position, then
                // push a single (y - current) impulse so velocity-based
                // animators glide smoothly to y.  ExpDecay variants chase
                // (anchor + Σδ = y) the same way.
                let now = std::time::Instant::now();
                let current = self
                    .scroll
                    .current_position(now)
                    .clamp(0.0, self.scroll.img_h as f64);
                let was_settled = !self.scroll.animator.is_animating(
                    self.scroll.anchor,
                    &self.scroll.input_history,
                    now,
                );
                self.scroll.input_history.drain();
                self.scroll.anchor = current;
                let delta = (y as i32) - (current.round() as i32);
                let direction = if delta >= 0 {
                    super::input_history::ScrollDirection::Down
                } else {
                    super::input_history::ScrollDirection::Up
                };
                let _ = self.scroll.input_history.record(direction, delta);
                self.scroll.animator.restart_ease_in_if_settled(was_settled);
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
            Effect::AccumulateZoom(delta) => {
                self.accumulate_zoom_delta(delta);
            }
        }
        (self, ops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulate_zoom_delta_increments_pending_and_marks_dirty() {
        let mut vp = Viewport::default();
        assert_eq!(vp.pending_zoom_delta, 0);
        assert!(!vp.dirty);

        vp.accumulate_zoom_delta(2);
        assert_eq!(vp.pending_zoom_delta, 2);
        assert!(vp.dirty);

        vp.accumulate_zoom_delta(-3);
        assert_eq!(vp.pending_zoom_delta, -1);
        assert!(vp.dirty);
    }

    #[test]
    fn accumulate_zoom_delta_saturates_on_overflow() {
        let mut vp = Viewport::default();
        vp.accumulate_zoom_delta(i32::MAX);
        // Adding another positive delta must not panic; saturating_add clamps.
        vp.accumulate_zoom_delta(10);
        assert_eq!(vp.pending_zoom_delta, i32::MAX);
    }
}
