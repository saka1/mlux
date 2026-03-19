//! Screen state and transition logic for the viewer.
//!
//! `Viewport` is the user's interactive view into a document build — a value-typed
//! state machine whose `apply` method is a pure transition function returning
//! `(Self, Vec<RenderOp>)`.

use log::debug;
use std::path::{Path, PathBuf};

use crate::input_source::InputSource;

use super::display_state::DisplayState;
use super::effect::{Effect, ExitReason, RenderOp, ViewerMode};
use super::layout::ScrollState;
use super::mode_log::LogState;
use super::mode_search::LastSearch;
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
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            mode: ViewerMode::Normal,
            scroll: ScrollState {
                y_offset: 0,
                img_h: 0,
                vp_w: 0,
                vp_h: 0,
            },
            display: DisplayState::new(0),
            flash: None,
            dirty: false,
            last_search: None,
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
    pub input: &'a InputSource,
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
                // Snap to cell_h boundary so that in the Split case of
                // place_tiles, top_src_h is always a multiple of cell_h
                // (prevents compression artifacts from round() mismatch).
                let cell_h = ctx.layout.cell_h as u32;
                let snapped = (y / cell_h) * cell_h;
                if snapped != y {
                    debug!("scroll snap: {y} -> {snapped} (cell_h={cell_h})");
                }
                self.scroll.y_offset = snapped;
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
            Effect::RedrawSearch => {
                if matches!(self.mode, ViewerMode::Search(_)) {
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
            Effect::RedrawLog => {
                if matches!(self.mode, ViewerMode::Log(_)) {
                    ops.push(RenderOp::DrawModeScreen);
                }
            }
            Effect::Yank(text) => {
                ops.push(RenderOp::CopyToClipboard(text));
            }
            Effect::OpenUrl(url) => {
                if let InputSource::File(cur) = ctx.input
                    && is_local_markdown_link(&url)
                    && let Some(path) = resolve_link_path(&url, cur)
                {
                    ops.push(RenderOp::Exit(ExitReason::Navigate { path }));
                } else {
                    ops.push(RenderOp::OpenExternal(url));
                }
            }
            Effect::SetMode(m) => {
                match &m {
                    ViewerMode::Search(_)
                    | ViewerMode::Command(_)
                    | ViewerMode::UrlPicker(_)
                    | ViewerMode::Toc(_)
                    | ViewerMode::Log(_) => {
                        ops.push(RenderOp::DrawModeScreen);
                    }
                    ViewerMode::Normal => {
                        // Clear text from search/command screen and
                        // purge terminal-side image data so that
                        // ensure_loaded() re-uploads from cache.
                        ops.push(RenderOp::ClearScreen);
                        ops.push(RenderOp::DeleteAllImages);
                        self.display.clear_all();
                        self.dirty = true;
                    }
                }
                self.mode = m;
            }
            Effect::SetLastSearch(ls) => {
                self.last_search = Some(ls);
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
                        self.display.map.clear();
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
            Effect::Exit(reason) => {
                ops.push(RenderOp::Exit(reason));
            }
        }
        (self, ops)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a URL points to a local markdown file (not a web URL).
fn is_local_markdown_link(url: &str) -> bool {
    if url.contains("://") || url.starts_with("mailto:") {
        return false;
    }
    let path_part = url.split('#').next().unwrap_or(url);
    path_part.ends_with(".md") || path_part.ends_with(".markdown")
}

/// Resolve a relative link URL against the current file's directory.
fn resolve_link_path(url: &str, current_file: &Path) -> Option<PathBuf> {
    let path_part = url.split('#').next().unwrap_or(url);
    if path_part.is_empty() {
        return None;
    }
    let base_dir = current_file.parent()?;
    Some(base_dir.join(path_part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_local_markdown_link() {
        assert!(is_local_markdown_link("./other.md"));
        assert!(is_local_markdown_link("other.md"));
        assert!(is_local_markdown_link("../docs/guide.md"));
        assert!(is_local_markdown_link("file.md#section"));
        assert!(is_local_markdown_link("notes.markdown"));

        assert!(!is_local_markdown_link("https://example.com"));
        assert!(!is_local_markdown_link("http://example.com/page.md"));
        assert!(!is_local_markdown_link("mailto:user@example.com"));
        assert!(!is_local_markdown_link("data.csv"));
        assert!(!is_local_markdown_link("image.png"));
        assert!(!is_local_markdown_link(""));
    }

    #[test]
    fn test_resolve_link_path() {
        let current = Path::new("/home/user/docs/readme.md");

        assert_eq!(
            resolve_link_path("other.md", current),
            Some(PathBuf::from("/home/user/docs/other.md"))
        );
        assert_eq!(
            resolve_link_path("../guide.md", current),
            Some(PathBuf::from("/home/user/docs/../guide.md"))
        );
        assert_eq!(
            resolve_link_path("sub/page.md#heading", current),
            Some(PathBuf::from("/home/user/docs/sub/page.md"))
        );
        // Fragment-only link -> None
        assert_eq!(resolve_link_path("#heading", current), None);
        // Empty -> None
        assert_eq!(resolve_link_path("", current), None);
    }
}
