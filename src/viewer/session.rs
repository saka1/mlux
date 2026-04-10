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
