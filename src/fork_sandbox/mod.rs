//! Fork-based sandboxed primitives.
//!
//! Provides generic fork+sandbox+IPC building blocks used by the
//! [`crate::renderer`] orchestration layer. This module contains no
//! domain-specific logic (no Markdown, no Typst, no tile rendering).

pub(crate) mod process;
pub(crate) mod sandbox;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::log::{LogBuffer, LogEntry};

/// Result from a forked child: either a computed value or a panic notification.
/// Both variants carry the child's log entries for forwarding to the parent.
#[derive(Serialize, Deserialize)]
enum ComputeResult<T> {
    Ok { value: T, logs: Vec<LogEntry> },
    Panicked { logs: Vec<LogEntry> },
}

pub use process::ChildProcess;

/// Convenience wrapper for tests: `fork_compute` with no sandbox.
#[cfg(test)]
fn fork_compute_nosandbox<T, F>(log_buffer: &LogBuffer, f: F) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> T,
{
    fork_compute(None, &[], true, log_buffer, f)
}

/// Fork a sandboxed child that computes a value and returns it.
///
/// The child applies Landlock, runs `f()`, sends the result via IPC, and exits.
/// Panics in `f` are caught; child log entries are forwarded in both cases.
pub fn fork_compute<T, F>(
    sandbox_read_base: Option<&Path>,
    font_dirs: &[PathBuf],
    no_sandbox: bool,
    log_buffer: &LogBuffer,
    f: F,
) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> T,
{
    let sandbox_base: Option<PathBuf> = sandbox_read_base.map(|p| p.to_path_buf());
    let font_dirs = font_dirs.to_vec();
    let log_buf = log_buffer.clone();
    let (_, mut rx, mut child) =
        process::fork_with_channels::<(), ComputeResult<T>, _>(move |_req_rx, mut resp_tx| {
            if !no_sandbox
                && let Err(e) = sandbox::enforce_sandbox(sandbox_base.as_deref(), &font_dirs)
            {
                log::warn!("child: sandbox failed: {e:#}");
            }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                Ok(value) => {
                    let logs = log_buf.drain();
                    let _ = resp_tx.send(&ComputeResult::Ok { value, logs });
                }
                Err(_) => {
                    log::error!("child: fork_compute panicked");
                    let logs = log_buf.drain();
                    let _ = resp_tx.send(&ComputeResult::<T>::Panicked { logs });
                }
            }
        })?;
    let result = rx.recv().context("fork_compute: child failed")?;
    match result {
        ComputeResult::Ok { value, logs } => {
            for entry in logs {
                log_buffer.push(entry);
            }
            child.wait()?;
            Ok(value)
        }
        ComputeResult::Panicked { logs } => {
            for entry in logs {
                log_buffer.push(entry);
            }
            child.wait()?;
            anyhow::bail!("fork_compute: child panicked")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_compute_ok_returns_value() {
        let log_buf = LogBuffer::new(16);
        let result = fork_compute_nosandbox(&log_buf, || 42u64);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn fork_compute_panic_returns_error() {
        let log_buf = LogBuffer::new(16);
        let result = fork_compute_nosandbox::<String, _>(&log_buf, || {
            panic!("deliberate test panic");
        });
        let err = result.unwrap_err();
        assert!(
            format!("{err:#}").contains("panicked"),
            "expected panic error, got: {err:#}"
        );
    }
}
