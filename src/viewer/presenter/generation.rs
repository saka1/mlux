//! Double-buffer generation tracking for reload without flicker.
//!
//! Two fixed image-ID ranges (bases 100 and 5000) are toggled on reload so
//! old-generation images stay visible on screen while new tiles compile and
//! upload. `GenerationTracker` owns this state and provides a minimal API
//! so the outer viewer loop doesn't need to know about ID ranges or stale lists.

use super::super::terminal;
use super::TilePresenter;

const GEN_BASES: [u32; 2] = [100, 5000];

/// Manages the double-buffer generation lifecycle.
///
/// Owned by the outer `'outer` loop in `src/viewer/mod.rs`. Lives across
/// Viewport rebuilds (which happen on every reload/resize) so generation
/// state is preserved even when `Viewport` itself is re-created.
pub(in super::super) struct GenerationTracker {
    active: usize,
    stale: Vec<u32>,
}

impl GenerationTracker {
    pub(in super::super) fn new() -> Self {
        Self {
            active: 0,
            stale: Vec::new(),
        }
    }

    /// The image-ID base for the currently-active generation.
    pub(in super::super) fn current_base(&self) -> u32 {
        GEN_BASES[self.active]
    }

    /// Snapshot all image IDs from the old generation so they can be
    /// cleaned up after the new generation renders.
    ///
    /// Call just before exiting the inner loop on `Reload`.
    pub(in super::super) fn capture_stale(&mut self, presenter: &TilePresenter) {
        self.stale = presenter.all_image_ids();
    }

    /// Whether there are stale images pending cleanup.
    ///
    /// Used to decide whether to show a loading screen that clears the
    /// viewport (we skip the clear when old-gen tiles are still displayed).
    pub(in super::super) fn has_stale(&self) -> bool {
        !self.stale.is_empty()
    }

    /// Delete stale images from the terminal and clear the list.
    ///
    /// Call after the new generation has successfully rendered its first frame.
    pub(in super::super) fn finalize(&mut self) -> anyhow::Result<()> {
        if !self.stale.is_empty() {
            terminal::delete_images_by_ids(&self.stale)?;
            self.stale.clear();
        }
        Ok(())
    }

    /// Advance generation state at the end of each outer-loop iteration.
    ///
    /// - `Reload`: toggle the active generation, keeping stale IDs for cleanup.
    /// - Any other exit: reset to generation 0 and discard stale IDs
    ///   (caller already called `delete_all_images` or a resize cleared everything).
    pub(in super::super) fn on_exit(&mut self, is_reload: bool) {
        if is_reload {
            self.active ^= 1;
        } else {
            self.active = 0;
            self.stale.clear();
        }
    }
}
