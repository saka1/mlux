//! Persistent session state across document rebuilds.
//!
//! `Session` survives document rebuilds (resize, reload, navigation) and holds
//! configuration, file management, and navigation history.

use crossterm::terminal as crossterm_terminal;
use log::debug;
use std::path::{Path, PathBuf};

use crate::input_source::InputSource;
use crate::watch::FileWatcher;

use super::layout::{self, Layout};
use super::terminal;

/// Jump stack entry for markdown link navigation.
pub(super) struct JumpEntry {
    pub path: PathBuf,
    pub y_offset: u32,
}

/// Scales a pixel scroll offset for a zoom transition so that the same
/// document position stays anchored at the top of the viewport.
fn scale_scroll(scroll: u32, old_scale: f64, new_scale: f64) -> u32 {
    if old_scale <= 0.0 || !new_scale.is_finite() {
        return scroll;
    }
    let scaled = (scroll as f64 * (new_scale / old_scale)).round();
    scaled.clamp(0.0, u32::MAX as f64) as u32
}

/// Persistent viewing session from viewer start to exit.
///
/// Survives document rebuilds (resize, reload, navigation). Contains
/// configuration, file management, and navigation history.
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
    /// The current file path, if the input is a file (not stdin).
    pub(super) fn current_file_path(&self) -> Option<&Path> {
        match &self.input {
            InputSource::File(p) => Some(p),
            InputSource::Stdin(_) => None,
        }
    }

    /// Recompute layout for new terminal dimensions and clear stale images.
    pub(super) fn update_layout_for_resize(
        &mut self,
        new_cols: u16,
        new_rows: u16,
        sidebar_cols: u16,
    ) -> anyhow::Result<()> {
        let new_winsize = crossterm_terminal::window_size()?;
        self.layout = layout::compute_layout(
            new_cols,
            new_rows,
            new_winsize.width,
            new_winsize.height,
            sidebar_cols,
        );
        terminal::delete_all_images()?;
        Ok(())
    }

    /// Handle an exit reason from the inner loop, returning `true` if the outer loop should break.
    pub(super) fn handle_exit(
        &mut self,
        exit: super::effect::ExitReason,
        scroll_position: u32,
        sidebar_cols: u16,
    ) -> anyhow::Result<bool> {
        use super::effect::ExitReason;
        match exit {
            ExitReason::Quit => return Ok(true),
            ExitReason::Resize { new_cols, new_rows } => {
                self.scroll_carry = scroll_position;
                debug!("resize: rebuilding tiled document and sidebar");
                self.update_layout_for_resize(new_cols, new_rows, sidebar_cols)?;
            }
            ExitReason::Reload => {
                self.scroll_carry = scroll_position;
                debug!("file changed: reloading document (double-buffer swap)");
                // Old images are NOT deleted here — they stay visible while
                // the new document compiles. Cleanup happens after the first
                // redraw of the new generation.
            }
            ExitReason::SetScale { old, new, flash } => {
                self.scroll_carry = scale_scroll(scroll_position, old, new);
                if flash.is_some() {
                    self.pending_flash = flash;
                }
                debug!(
                    "scale changed {old} → {new}: scroll {scroll_position} → {} (rebuilding, double-buffer swap)",
                    self.scroll_carry
                );
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

#[cfg(test)]
mod tests {
    use super::scale_scroll;

    #[test]
    fn scale_scroll_zoom_in_doubles_offset() {
        assert_eq!(scale_scroll(1000, 1.0, 2.0), 2000);
    }

    #[test]
    fn scale_scroll_zoom_out_halves_offset() {
        assert_eq!(scale_scroll(1000, 2.0, 1.0), 500);
    }

    #[test]
    fn scale_scroll_no_change_is_identity() {
        assert_eq!(scale_scroll(1234, 1.25, 1.25), 1234);
    }

    #[test]
    fn scale_scroll_zero_is_zero() {
        assert_eq!(scale_scroll(0, 1.0, 2.0), 0);
    }

    #[test]
    fn scale_scroll_rounds_to_nearest() {
        // 100 * (0.85 / 1.0) = 85.0
        assert_eq!(scale_scroll(100, 1.0, 0.85), 85);
        // 100 * (1.5 / 0.85) = 176.470… → 176
        assert_eq!(scale_scroll(100, 0.85, 1.5), 176);
    }

    #[test]
    fn scale_scroll_handles_invalid_old_scale() {
        // Defensive: never divide by zero / negatives.
        assert_eq!(scale_scroll(500, 0.0, 2.0), 500);
        assert_eq!(scale_scroll(500, -1.0, 2.0), 500);
    }

    #[test]
    fn scale_scroll_clamps_overflow() {
        // u32::MAX * 2.0 overflows u32; must saturate.
        assert_eq!(scale_scroll(u32::MAX, 1.0, 2.0), u32::MAX);
    }
}
