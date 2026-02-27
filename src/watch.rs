//! File watcher — monitors a file for changes via notify (inotify on Linux).
//!
//! notify::RecommendedWatcher runs callbacks on an internal thread.
//! FileWatcher bridges change notifications to the main thread via mpsc::channel.

use std::path::Path;
use std::sync::mpsc;

use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub struct FileWatcher {
    rx: mpsc::Receiver<()>,
    _watcher: RecommendedWatcher, // Drop stops watching
}

impl FileWatcher {
    /// Create a FileWatcher that monitors the given file for changes.
    ///
    /// Linux inotify loses the watch on rename (atomic save), so we watch
    /// the parent directory (NonRecursive) and filter events by path.
    pub fn new(path: &Path) -> Result<Self> {
        let canonical = path.canonicalize()?;
        let target = canonical.clone();
        let (tx, rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let dominated = event.paths.iter().any(|p| p == &target);
                    if dominated && event.kind.is_modify() {
                        let _ = tx.send(());
                    }
                }
            },
            notify::Config::default(),
        )?;
        let parent = canonical
            .parent()
            .ok_or_else(|| anyhow::anyhow!("cannot watch root path"))?;
        watcher.watch(parent, RecursiveMode::NonRecursive)?;

        Ok(Self { rx, _watcher: watcher })
    }

    /// Return true if the file has changed since last check (non-blocking).
    /// Multiple queued notifications are collapsed into a single true.
    pub fn has_changed(&self) -> bool {
        let mut changed = false;
        while self.rx.try_recv().is_ok() {
            changed = true;
        }
        changed
    }
}
