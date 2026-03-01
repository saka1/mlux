//! Input source abstraction: file path or stdin pipe with incremental reading.
//!
//! `StdinReader` spawns a reader thread that sends chunks via `mpsc::channel`,
//! avoiding platform-specific non-blocking I/O. The main thread drains with
//! `try_recv()` at each poll cycle, naturally coalescing rapid input.

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

/// Input source for the viewer/renderer.
pub enum InputSource {
    File(PathBuf),
    Stdin(StdinReader),
}

impl InputSource {
    /// Display name for the status bar.
    pub fn display_name(&self) -> &str {
        match self {
            InputSource::File(path) => path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown"),
            InputSource::Stdin(_) => "<stdin>",
        }
    }
}

/// A chunk of data from the stdin reader thread.
enum StdinChunk {
    Data(String),
    Eof,
}

/// Result of draining the stdin channel.
pub struct DrainResult {
    /// Whether any new data (or EOF) was received.
    pub got_data: bool,
    /// Whether EOF has been reached.
    pub eof: bool,
}

/// Reads stdin in a background thread, sending chunks over a channel.
pub struct StdinReader {
    rx: mpsc::Receiver<StdinChunk>,
    _handle: JoinHandle<()>,
}

impl Default for StdinReader {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinReader {
    /// Spawn the reader thread and return a new `StdinReader`.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(StdinChunk::Eof);
                        break;
                    }
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&buf[..n]).into_owned();
                        if tx.send(StdinChunk::Data(s)).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        let _ = tx.send(StdinChunk::Eof);
                        break;
                    }
                }
            }
        });

        Self {
            rx,
            _handle: handle,
        }
    }

    /// Drain all available chunks into `buf`. Non-blocking.
    pub fn drain_into(&self, buf: &mut String) -> DrainResult {
        let mut got_data = false;
        let mut eof = false;
        while let Ok(chunk) = self.rx.try_recv() {
            match chunk {
                StdinChunk::Data(s) => {
                    buf.push_str(&s);
                    got_data = true;
                }
                StdinChunk::Eof => {
                    eof = true;
                    got_data = true;
                }
            }
        }
        DrainResult { got_data, eof }
    }
}

/// Detect whether the given CLI input argument represents stdin.
///
/// Returns `true` if input is `Some("-")`, or if input is `None` and stdin is not a terminal.
pub fn is_stdin_input(input: Option<&Path>) -> bool {
    match input {
        Some(p) => p.as_os_str() == "-",
        None => !io::stdin().is_terminal(),
    }
}

/// Read all of stdin to a string (blocking, for render mode).
pub fn read_stdin_to_string() -> io::Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}
