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

use crate::log::{LogBuffer, LogEntry, WireLogEntry};

/// Wrapper for fork_compute results that includes child-process log entries.
#[derive(Serialize, Deserialize)]
struct ComputeResult<T> {
    value: T,
    logs: Vec<WireLogEntry>,
}

pub use process::ChildProcess;

/// Fork a sandboxed child that computes a value and returns it.
///
/// The child applies Landlock, runs `f()`, sends the result via IPC, and exits.
/// Panics in `f` are caught and converted to an error on the parent side
/// (child exits, pipe closes, parent recv fails).
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
                    let logs = log_buf
                        .drain()
                        .into_iter()
                        .map(WireLogEntry::from)
                        .collect();
                    let _ = resp_tx.send(&ComputeResult { value, logs });
                }
                Err(_) => {
                    log::error!("child: fork_compute panicked");
                    // Logs are lost here: we cannot send ComputeResult without
                    // a value (T has no Default), and the pipe close signals the
                    // error to the parent. Accepted trade-off per design doc.
                }
            }
        })?;
    let result = rx.recv().context("fork_compute: child failed")?;
    for entry in result.logs {
        log_buffer.push(LogEntry::from(entry));
    }
    child.wait()?;
    Ok(result.value)
}
