//! Effect types, viewer mode, exit reasons, and effect application logic.
//!
//! State model:
//!   - `Viewport`: mutable state for the current document view (mode, scroll, tiles, UI)
//!   - `ViewContext`: read-only environment for effect application (layout, document, nav)
//!   - `Session`: persistent state across document rebuilds (config, file mgmt, nav history)
//!
//! Layout geometry lives in `layout.rs`, tile cache in `tiles.rs`.
//! See `docs/2026-03-07-design-viewer-state.md` for the full design rationale.

use crossterm::terminal as crossterm_terminal;
use log::debug;
use std::path::{Path, PathBuf};

use crate::config::{self, CliOverrides, Config};
use crate::input::InputSource;
use crate::tile::VisualLine;
use crate::watch::FileWatcher;

use super::layout::{self, Layout, ScrollState};
use super::mode_command::CommandState;
use super::mode_search::{self, LastSearch, SearchState};
use super::mode_toc::{self, TocState};
use super::mode_url::{self, UrlPickerState};
use super::terminal;
use super::tiles::LoadedTiles;

/// Why the inner event loop exited back to the outer rebuild loop.
pub(super) enum ExitReason {
    Quit,
    Resize { new_cols: u16, new_rows: u16 },
    Reload,
    ConfigReload,
    Navigate { path: PathBuf },
    GoBack,
}

/// Viewer mode: normal (tile display), search (picker UI), command (`:` prompt), or URL picker.
pub(super) enum ViewerMode {
    Normal,
    Search(SearchState),
    Command(CommandState),
    UrlPicker(UrlPickerState),
    Toc(TocState),
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
    RedrawSearch,
    RedrawCommandBar,
    RedrawUrlPicker,
    RedrawToc,
    Yank(String),
    OpenUrl(String),
    SetMode(ViewerMode),
    SetLastSearch(LastSearch),
    DeletePlacements,
    EnterUrlPickerAll,
    GoBack,
    Exit(ExitReason),
}

/// Jump stack entry for markdown link navigation.
pub(super) struct JumpEntry {
    pub path: PathBuf,
    pub y_offset: u32,
}

// ---------------------------------------------------------------------------
// Viewport — the user's interactive view into a document
// ---------------------------------------------------------------------------

/// The user's interactive view into a document build.
///
/// Contains all mutable state for the inner event loop: interaction mode,
/// scroll position, tile cache, and transient UI state. Created fresh
/// for each document build; destroyed when the build is replaced.
pub(super) struct Viewport {
    pub mode: ViewerMode,
    pub scroll: ScrollState,
    pub tiles: LoadedTiles,
    pub flash: Option<String>,
    pub dirty: bool,
    pub last_search: Option<LastSearch>,
}

/// Read-only environment for effect application.
///
/// Bundles references to layout, document content, and navigation state
/// that `Viewport::apply` needs but must not modify.
pub(super) struct ViewContext<'a> {
    pub layout: &'a Layout,
    pub acc_value: Option<u32>,
    pub input: &'a InputSource,
    pub filename: &'a str,
    pub jump_stack: &'a [JumpEntry],
    pub markdown: &'a str,
    pub visual_lines: &'a [VisualLine],
}

impl Viewport {
    /// Apply a single effect, returning an `ExitReason` if the inner loop should exit.
    pub(super) fn apply(
        &mut self,
        effect: Effect,
        ctx: &ViewContext,
    ) -> anyhow::Result<Option<ExitReason>> {
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
                terminal::draw_status_bar(
                    ctx.layout,
                    &self.scroll,
                    ctx.filename,
                    ctx.acc_value,
                    self.flash.as_deref(),
                )?;
            }
            Effect::RedrawSearch => {
                if let ViewerMode::Search(ss) = &self.mode {
                    mode_search::draw_search_screen(
                        ctx.layout,
                        &ss.query,
                        &ss.matches,
                        ss.selected,
                        ss.scroll_offset,
                        ss.pattern_valid,
                    )?;
                }
            }
            Effect::RedrawCommandBar => {
                if let ViewerMode::Command(cs) = &self.mode {
                    terminal::draw_command_bar(ctx.layout, &cs.input)?;
                }
            }
            Effect::RedrawUrlPicker => {
                if let ViewerMode::UrlPicker(up) = &self.mode {
                    mode_url::draw_url_screen(ctx.layout, up)?;
                }
            }
            Effect::RedrawToc => {
                if let ViewerMode::Toc(ts) = &self.mode {
                    mode_toc::draw_toc_screen(ctx.layout, ts)?;
                }
            }
            Effect::Yank(text) => {
                let _ = terminal::send_osc52(&text);
            }
            Effect::OpenUrl(url) => {
                if let InputSource::File(cur) = ctx.input
                    && is_local_markdown_link(&url)
                    && let Some(path) = resolve_link_path(&url, cur)
                {
                    return Ok(Some(ExitReason::Navigate { path }));
                }
                let _ = open::that_in_background(&url);
            }
            Effect::SetMode(m) => {
                match &m {
                    ViewerMode::Search(ss) => {
                        mode_search::draw_search_screen(
                            ctx.layout,
                            &ss.query,
                            &ss.matches,
                            ss.selected,
                            ss.scroll_offset,
                            ss.pattern_valid,
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
                    ViewerMode::Normal => {
                        // Clear text from search/command screen and
                        // purge terminal-side image data so that
                        // ensure_loaded() re-uploads from cache.
                        terminal::clear_screen()?;
                        terminal::delete_all_images()?;
                        self.tiles.map.clear();
                        self.dirty = true;
                    }
                }
                self.mode = m;
            }
            Effect::SetLastSearch(ls) => {
                self.last_search = Some(ls);
            }
            Effect::DeletePlacements => {
                self.tiles.delete_placements()?;
            }
            Effect::EnterUrlPickerAll => {
                let entries = mode_url::collect_all_url_entries(ctx.markdown, ctx.visual_lines);
                if entries.is_empty() {
                    // Return to normal with flash; need full
                    // redraw if coming from command mode.
                    self.flash = Some("No URLs in document".into());
                    if !matches!(self.mode, ViewerMode::Normal) {
                        terminal::clear_screen()?;
                        terminal::delete_all_images()?;
                        self.tiles.map.clear();
                        self.mode = ViewerMode::Normal;
                        self.dirty = true;
                    } else {
                        terminal::draw_status_bar(
                            ctx.layout,
                            &self.scroll,
                            ctx.filename,
                            ctx.acc_value,
                            self.flash.as_deref(),
                        )?;
                    }
                } else {
                    self.tiles.delete_placements()?;
                    terminal::clear_screen()?;
                    let up = UrlPickerState::new(entries);
                    mode_url::draw_url_screen(ctx.layout, &up)?;
                    self.mode = ViewerMode::UrlPicker(up);
                }
            }
            Effect::GoBack => {
                if ctx.jump_stack.is_empty() {
                    self.flash = Some("No previous file".into());
                    terminal::draw_status_bar(
                        ctx.layout,
                        &self.scroll,
                        ctx.filename,
                        ctx.acc_value,
                        self.flash.as_deref(),
                    )?;
                } else {
                    return Ok(Some(ExitReason::GoBack));
                }
            }
            Effect::Exit(reason) => {
                return Ok(Some(reason));
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Session — persistent state across document rebuilds
// ---------------------------------------------------------------------------

/// Persistent viewing session from viewer start to exit.
///
/// Survives document rebuilds (resize, reload, navigation). Contains
/// configuration, file management, and navigation history.
pub(super) struct Session {
    pub layout: Layout,
    pub config: Config,
    pub cli_overrides: CliOverrides,
    pub input: InputSource,
    pub filename: String,
    pub watcher: Option<FileWatcher>,
    pub jump_stack: Vec<JumpEntry>,
    pub scroll_carry: u32,
    pub pending_flash: Option<String>,
    pub watch: bool,
    pub detected_light: bool,
}

impl Session {
    /// Recompute layout for new terminal dimensions and clear stale images.
    pub(super) fn update_layout_for_resize(
        &mut self,
        new_cols: u16,
        new_rows: u16,
    ) -> anyhow::Result<()> {
        let new_winsize = crossterm_terminal::window_size()?;
        self.layout = layout::compute_layout(
            new_cols,
            new_rows,
            new_winsize.width,
            new_winsize.height,
            self.config.viewer.sidebar_cols,
        );
        terminal::delete_all_images()?;
        Ok(())
    }

    /// Handle an exit reason from the inner loop, returning `true` if the outer loop should break.
    pub(super) fn handle_exit(
        &mut self,
        exit: ExitReason,
        scroll_position: u32,
    ) -> anyhow::Result<bool> {
        match exit {
            ExitReason::Quit => return Ok(true),
            ExitReason::Resize { new_cols, new_rows } => {
                self.scroll_carry = scroll_position;
                debug!("resize: rebuilding tiled document and sidebar");
                self.update_layout_for_resize(new_cols, new_rows)?;
            }
            ExitReason::Reload => {
                self.scroll_carry = scroll_position;
                debug!("file changed: reloading document");
                terminal::delete_all_images()?;
            }
            ExitReason::ConfigReload => {
                self.scroll_carry = scroll_position;
                debug!("config reload requested");

                match config::reload_config(&self.cli_overrides) {
                    Ok(new_config) => {
                        // Verify built-in theme exists before committing
                        let resolved = crate::theme::resolve_theme_name(
                            &new_config.theme,
                            self.detected_light,
                        );
                        if crate::theme::get(resolved).is_none() {
                            self.pending_flash = Some(format!(
                                "Reload failed: unknown theme '{}'",
                                new_config.theme
                            ));
                            debug!(
                                "config reload: unknown theme '{}', keeping old config",
                                new_config.theme
                            );
                            // Rebuild with old config
                            terminal::delete_all_images()?;
                            return Ok(false);
                        }

                        // Recalculate layout if sidebar_cols changed
                        if new_config.viewer.sidebar_cols != self.config.viewer.sidebar_cols {
                            let winsize = crossterm_terminal::window_size()?;
                            self.layout = layout::compute_layout(
                                winsize.columns,
                                winsize.rows,
                                winsize.width,
                                winsize.height,
                                new_config.viewer.sidebar_cols,
                            );
                        }

                        self.config = new_config;
                        self.pending_flash = Some("Config reloaded".into());
                    }
                    Err(e) => {
                        self.pending_flash = Some(format!("Reload failed: {e}"));
                        debug!("config reload failed: {e}");
                        // Rebuild with old config
                    }
                }
                terminal::delete_all_images()?;
                // continue 'outer -> rebuild document with new (or old) config
            }
            ExitReason::Navigate { path } => {
                if !path.exists() {
                    self.pending_flash = Some(format!("File not found: {}", path.display()));
                    terminal::delete_all_images()?;
                    return Ok(false);
                }
                // Push current location onto jump stack
                if let InputSource::File(cur) = &self.input {
                    self.jump_stack.push(JumpEntry {
                        path: cur.clone(),
                        y_offset: scroll_position,
                    });
                }
                let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                debug!("navigate: jumping to {}", canonical.display());
                self.input = InputSource::File(canonical.clone());
                self.filename = self.input.display_name().to_string();
                self.scroll_carry = 0;
                if self.watch {
                    self.watcher = Some(FileWatcher::new(&canonical)?);
                }
                terminal::delete_all_images()?;
                // continue 'outer -> load new file
            }
            ExitReason::GoBack => {
                // jump_stack is guaranteed non-empty here (inner loop checks)
                let entry = self.jump_stack.pop().expect("GoBack with empty stack");
                debug!("go back: returning to {}", entry.path.display());
                self.input = InputSource::File(entry.path.clone());
                self.filename = self.input.display_name().to_string();
                self.scroll_carry = entry.y_offset;
                if self.watch {
                    self.watcher = Some(FileWatcher::new(&entry.path)?);
                }
                terminal::delete_all_images()?;
                // continue 'outer -> reload previous file
            }
        }
        Ok(false)
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
