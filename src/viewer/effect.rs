//! Effect types, viewer mode, and effect application logic.
//!
//! Extracted from mod.rs to reduce its size and isolate the effect
//! dispatch from the event loop.

use crossterm::terminal as crossterm_terminal;
use log::debug;
use std::path::{Path, PathBuf};

use crate::config::{self, CliOverrides, Config};
use crate::input::InputSource;
use crate::tile::VisualLine;
use crate::watch::FileWatcher;

use super::input::InputAccumulator;
use super::mode_command::CommandState;
use super::mode_search::{self, LastSearch, SearchState};
use super::mode_url::{self, UrlPickerState};
use super::state::{self, ExitReason, Layout, LoadedTiles, ViewState};
use super::terminal;

/// Viewer mode: normal (tile display), search (picker UI), command (`:` prompt), or URL picker.
pub(super) enum ViewerMode {
    Normal,
    Search(SearchState),
    Command(CommandState),
    UrlPicker(UrlPickerState),
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
    Yank(String),
    OpenUrl(String),
    SetMode(ViewerMode),
    SetLastSearch(LastSearch),
    DeletePlacements,
    EnterUrlPickerAll,
    GoBack,
    Exit(ExitReason),
}

/// Result of an async build attempt.
pub(super) enum BuildOutcome {
    Done(crate::tile::TiledDocument),
    Quit,
    Resize { new_cols: u16, new_rows: u16 },
}

/// Jump stack entry for markdown link navigation.
pub(super) struct JumpEntry {
    pub path: PathBuf,
    pub y_offset: u32,
}

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

/// Apply a single effect, returning an `ExitReason` if the inner loop should exit.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_effect(
    effect: Effect,
    mode: &mut ViewerMode,
    state: &mut ViewState,
    loaded: &mut LoadedTiles,
    layout: &Layout,
    acc: &InputAccumulator,
    flash_msg: &mut Option<String>,
    dirty: &mut bool,
    input: &InputSource,
    jump_stack: &[JumpEntry],
    last_search: &mut Option<LastSearch>,
    markdown: &str,
    visual_lines: &[VisualLine],
) -> anyhow::Result<Option<ExitReason>> {
    match effect {
        Effect::ScrollTo(y) => {
            state.y_offset = y;
            *dirty = true;
        }
        Effect::MarkDirty => {
            *dirty = true;
        }
        Effect::Flash(msg) => {
            *flash_msg = Some(msg);
        }
        Effect::RedrawStatusBar => {
            terminal::draw_status_bar(layout, state, acc.peek(), flash_msg.as_deref())?;
        }
        Effect::RedrawSearch => {
            if let ViewerMode::Search(ss) = &*mode {
                mode_search::draw_search_screen(
                    layout,
                    &ss.query,
                    &ss.matches,
                    ss.selected,
                    ss.scroll_offset,
                    ss.pattern_valid,
                )?;
            }
        }
        Effect::RedrawCommandBar => {
            if let ViewerMode::Command(cs) = &*mode {
                terminal::draw_command_bar(layout, &cs.input)?;
            }
        }
        Effect::RedrawUrlPicker => {
            if let ViewerMode::UrlPicker(up) = &*mode {
                mode_url::draw_url_screen(layout, up)?;
            }
        }
        Effect::Yank(text) => {
            let _ = terminal::send_osc52(&text);
        }
        Effect::OpenUrl(url) => {
            if let InputSource::File(cur) = input
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
                        layout,
                        &ss.query,
                        &ss.matches,
                        ss.selected,
                        ss.scroll_offset,
                        ss.pattern_valid,
                    )?;
                }
                ViewerMode::Command(cs) => {
                    terminal::draw_command_bar(layout, &cs.input)?;
                }
                ViewerMode::UrlPicker(up) => {
                    mode_url::draw_url_screen(layout, up)?;
                }
                ViewerMode::Normal => {
                    // Clear text from search/command screen and
                    // purge terminal-side image data so that
                    // ensure_loaded() re-uploads from cache.
                    terminal::clear_screen()?;
                    terminal::delete_all_images()?;
                    loaded.map.clear();
                    *dirty = true;
                }
            }
            *mode = m;
        }
        Effect::SetLastSearch(ls) => {
            *last_search = Some(ls);
        }
        Effect::DeletePlacements => {
            loaded.delete_placements()?;
        }
        Effect::EnterUrlPickerAll => {
            let entries = mode_url::collect_all_url_entries(markdown, visual_lines);
            if entries.is_empty() {
                // Return to normal with flash; need full
                // redraw if coming from command mode.
                *flash_msg = Some("No URLs in document".into());
                if !matches!(mode, ViewerMode::Normal) {
                    terminal::clear_screen()?;
                    terminal::delete_all_images()?;
                    loaded.map.clear();
                    *mode = ViewerMode::Normal;
                    *dirty = true;
                } else {
                    terminal::draw_status_bar(layout, state, acc.peek(), flash_msg.as_deref())?;
                }
            } else {
                loaded.delete_placements()?;
                terminal::clear_screen()?;
                let up = UrlPickerState::new(entries);
                mode_url::draw_url_screen(layout, &up)?;
                *mode = ViewerMode::UrlPicker(up);
            }
        }
        Effect::GoBack => {
            if jump_stack.is_empty() {
                *flash_msg = Some("No previous file".into());
                terminal::draw_status_bar(layout, state, acc.peek(), flash_msg.as_deref())?;
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

/// Handle an exit reason from the inner loop, returning `true` if the outer loop should break.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_exit_reason(
    exit: ExitReason,
    state: &ViewState,
    y_offset_carry: &mut u32,
    layout: &mut Layout,
    config: &mut Config,
    cli_overrides: &CliOverrides,
    input: &mut InputSource,
    filename: &mut String,
    watcher: &mut Option<FileWatcher>,
    jump_stack: &mut Vec<JumpEntry>,
    outer_flash: &mut Option<String>,
    watch: bool,
) -> anyhow::Result<bool> {
    match exit {
        ExitReason::Quit => return Ok(true),
        ExitReason::Resize { new_cols, new_rows } => {
            *y_offset_carry = state.y_offset;
            debug!("resize: rebuilding tiled document and sidebar");
            let new_winsize = crossterm_terminal::window_size()?;
            *layout = state::compute_layout(
                new_cols,
                new_rows,
                new_winsize.width,
                new_winsize.height,
                config.viewer.sidebar_cols,
            );
            terminal::delete_all_images()?;
        }
        ExitReason::Reload => {
            *y_offset_carry = state.y_offset;
            debug!("file changed: reloading document");
            terminal::delete_all_images()?;
        }
        ExitReason::ConfigReload => {
            *y_offset_carry = state.y_offset;
            debug!("config reload requested");

            match config::reload_config(cli_overrides) {
                Ok(new_config) => {
                    // Verify built-in theme exists before committing
                    if crate::theme::get(&new_config.theme).is_none() {
                        *outer_flash = Some(format!(
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
                    if new_config.viewer.sidebar_cols != config.viewer.sidebar_cols {
                        let winsize = crossterm_terminal::window_size()?;
                        *layout = state::compute_layout(
                            winsize.columns,
                            winsize.rows,
                            winsize.width,
                            winsize.height,
                            new_config.viewer.sidebar_cols,
                        );
                    }

                    *config = new_config;
                    *outer_flash = Some("Config reloaded".into());
                }
                Err(e) => {
                    *outer_flash = Some(format!("Reload failed: {e}"));
                    debug!("config reload failed: {e}");
                    // Rebuild with old config
                }
            }
            terminal::delete_all_images()?;
            // continue 'outer → rebuild document with new (or old) config
        }
        ExitReason::Navigate { path } => {
            if !path.exists() {
                *outer_flash = Some(format!("File not found: {}", path.display()));
                terminal::delete_all_images()?;
                return Ok(false);
            }
            // Push current location onto jump stack
            if let InputSource::File(cur) = &*input {
                jump_stack.push(JumpEntry {
                    path: cur.clone(),
                    y_offset: state.y_offset,
                });
            }
            let canonical = std::fs::canonicalize(&path).unwrap_or(path);
            debug!("navigate: jumping to {}", canonical.display());
            *input = InputSource::File(canonical.clone());
            *filename = input.display_name().to_string();
            *y_offset_carry = 0;
            if watch {
                *watcher = Some(FileWatcher::new(&canonical)?);
            }
            terminal::delete_all_images()?;
            // continue 'outer → load new file
        }
        ExitReason::GoBack => {
            // jump_stack is guaranteed non-empty here (inner loop checks)
            let entry = jump_stack.pop().expect("GoBack with empty stack");
            debug!("go back: returning to {}", entry.path.display());
            *input = InputSource::File(entry.path.clone());
            *filename = input.display_name().to_string();
            *y_offset_carry = entry.y_offset;
            if watch {
                *watcher = Some(FileWatcher::new(&entry.path)?);
            }
            terminal::delete_all_images()?;
            // continue 'outer → reload previous file
        }
    }
    Ok(false)
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
        // Fragment-only link → None
        assert_eq!(resolve_link_path("#heading", current), None);
        // Empty → None
        assert_eq!(resolve_link_path("", current), None);
    }
}
