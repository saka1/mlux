//! Fork-based sandboxed primitives.
//!
//! Provides generic fork+sandbox+IPC building blocks used by the
//! [`crate::usecase`] orchestration layer. This module contains no
//! domain-specific logic (no Markdown, no Typst, no tile rendering).

pub(crate) mod process;
pub(crate) mod sandbox;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

pub use process::ChildProcess;

/// Fork a sandboxed child that computes a value and returns it.
///
/// The child applies Landlock, runs `f()`, sends the result via IPC, and exits.
/// Panics in `f` are caught and converted to an error on the parent side
/// (child exits, pipe closes, parent recv fails).
pub fn fork_compute<T, F>(sandbox_read_base: Option<&Path>, no_sandbox: bool, f: F) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> T,
{
    let sandbox_base: Option<PathBuf> = sandbox_read_base.map(|p| p.to_path_buf());
    let (_, mut rx, mut child) =
        process::fork_with_channels::<(), T, _>(move |_req_rx, mut resp_tx| {
            if !no_sandbox && let Err(e) = sandbox::enforce_sandbox(sandbox_base.as_deref()) {
                log::warn!("child: sandbox failed: {e:#}");
            }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                Ok(result) => {
                    let _ = resp_tx.send(&result);
                }
                Err(_) => {
                    log::error!("child: fork_compute panicked");
                }
            }
        })?;
    let result = rx.recv().context("fork_compute: child failed")?;
    child.wait()?;
    Ok(result)
}
