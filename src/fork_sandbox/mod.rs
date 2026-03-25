//! Fork-based sandboxed primitives.
//!
//! Provides generic fork+sandbox+IPC building blocks used by the
//! [`crate::renderer`] orchestration layer. This module contains no
//! domain-specific logic (no Markdown, no Typst, no tile rendering).

mod process;
mod sandbox;

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::log::{LogBuffer, LogEntry};

pub use process::ChildProcess;
pub(crate) use process::{TypedReader, TypedWriter};

/// Sandbox policy for a forked child.
pub(crate) enum SandboxConfig {
    /// No filesystem/network restrictions.
    Disabled,
    /// Apply Landlock: restrict filesystem to listed read scopes, deny all network.
    Enforce {
        /// Base path for document access (expanded to git root by sandbox layer).
        read_base: Option<PathBuf>,
        /// Additional read-allowed directories (e.g. font dirs), used as-is.
        extra_read_dirs: Vec<PathBuf>,
    },
}

impl SandboxConfig {
    /// Maximum restriction: Landlock with no read scopes allowed.
    pub(crate) fn deny_all() -> Self {
        SandboxConfig::Enforce {
            read_base: None,
            extra_read_dirs: Vec::new(),
        }
    }
}

/// Fork a child with typed channels, applying sandbox before `child_fn` runs.
///
/// This is the general-purpose fork primitive of `fork_sandbox`.
/// If `sandbox` is `Enforce`, Landlock is applied before `child_fn` runs.
pub(crate) fn fork_sandboxed<Req, Resp, F>(
    sandbox: SandboxConfig,
    child_fn: F,
) -> Result<(TypedWriter<Req>, TypedReader<Resp>, ChildProcess)>
where
    Req: Serialize + DeserializeOwned,
    Resp: Serialize + DeserializeOwned,
    F: FnOnce(TypedReader<Req>, TypedWriter<Resp>),
{
    process::fork_with_channels(move |req_rx, resp_tx| {
        if let SandboxConfig::Enforce {
            ref read_base,
            ref extra_read_dirs,
        } = sandbox
            && let Err(e) = sandbox::enforce_sandbox(read_base.as_deref(), extra_read_dirs)
        {
            log::warn!("child: sandbox failed: {e:#}");
        }
        child_fn(req_rx, resp_tx);
    })
}

/// Result from a forked child: either a computed value or a panic notification.
/// Both variants carry the child's log entries for forwarding to the parent.
#[derive(Serialize, Deserialize)]
enum ComputeResult<T> {
    Ok { value: T, logs: Vec<LogEntry> },
    Panicked { logs: Vec<LogEntry> },
}

/// Convenience wrapper for tests: `fork_compute` with no sandbox.
#[cfg(test)]
fn fork_compute_nosandbox<T, F>(log_buffer: &LogBuffer, f: F) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> T,
{
    fork_compute(SandboxConfig::Disabled, log_buffer, f)
}

/// Fork a sandboxed child that computes a value and returns it.
///
/// The child applies the sandbox policy, runs `f()`, sends the result via IPC,
/// and exits. Panics in `f` are caught; child log entries are forwarded in both cases.
pub(crate) fn fork_compute<T, F>(sandbox: SandboxConfig, log_buffer: &LogBuffer, f: F) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> T,
{
    let log_buf = log_buffer.clone();
    let (_, mut rx, mut child) =
        fork_sandboxed::<(), ComputeResult<T>, _>(sandbox, move |_req_rx, mut resp_tx| {
            // Discard log entries inherited from the parent via fork COW.
            // Without this, drain() would include pre-fork entries, causing
            // duplicates when the parent re-ingests them.
            log_buf.drain();
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
