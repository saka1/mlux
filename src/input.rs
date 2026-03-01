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
        Self::from_reader(io::stdin())
    }

    /// Spawn a reader thread from an arbitrary `Read` source.
    fn from_reader<R: Read + Send + 'static>(mut reader: R) -> Self {
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    // --- Test helpers ---

    /// Drain repeatedly until EOF is received.
    fn drain_until_eof(reader: &StdinReader, buf: &mut String) {
        loop {
            let result = reader.drain_into(buf);
            if result.eof {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Reader that returns `Interrupted` on the first call, then delegates to inner.
    struct InterruptedReader {
        interrupted: bool,
        inner: Cursor<Vec<u8>>,
    }

    impl InterruptedReader {
        fn new(data: &[u8]) -> Self {
            Self {
                interrupted: false,
                inner: Cursor::new(data.to_vec()),
            }
        }
    }

    impl Read for InterruptedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
            }
            self.inner.read(buf)
        }
    }

    /// Reader that always returns a BrokenPipe error.
    struct ErrorReader;

    impl Read for ErrorReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
        }
    }

    // --- display_name tests ---

    #[test]
    fn display_name_simple_file() {
        let src = InputSource::File(PathBuf::from("readme.md"));
        assert_eq!(src.display_name(), "readme.md");
    }

    #[test]
    fn display_name_nested_path() {
        let src = InputSource::File(PathBuf::from("/home/user/docs/notes.md"));
        assert_eq!(src.display_name(), "notes.md");
    }

    #[test]
    fn display_name_no_filename() {
        let src = InputSource::File(PathBuf::from("/"));
        assert_eq!(src.display_name(), "unknown");
    }

    #[test]
    fn display_name_stdin() {
        let reader = StdinReader::from_reader(Cursor::new(b""));
        let src = InputSource::Stdin(reader);
        assert_eq!(src.display_name(), "<stdin>");
    }

    // --- is_stdin_input tests ---
    // Note: `None` case depends on terminal state (`is_terminal()`), so we skip it.

    #[test]
    fn is_stdin_input_dash() {
        assert!(is_stdin_input(Some(Path::new("-"))));
    }

    #[test]
    fn is_stdin_input_file_path() {
        assert!(!is_stdin_input(Some(Path::new("file.md"))));
    }

    #[test]
    fn is_stdin_input_empty_string() {
        assert!(!is_stdin_input(Some(Path::new(""))));
    }

    // --- StdinReader + from_reader tests ---

    #[test]
    fn reader_single_chunk() {
        let reader = StdinReader::from_reader(Cursor::new(b"hello world"));
        let mut buf = String::new();
        drain_until_eof(&reader, &mut buf);
        assert_eq!(buf, "hello world");
    }

    #[test]
    fn reader_empty_input() {
        let reader = StdinReader::from_reader(Cursor::new(b""));
        let mut buf = String::new();
        drain_until_eof(&reader, &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn reader_large_data() {
        let data = "x".repeat(20_000);
        let reader = StdinReader::from_reader(Cursor::new(data.clone().into_bytes()));
        let mut buf = String::new();
        drain_until_eof(&reader, &mut buf);
        assert_eq!(buf.len(), 20_000);
        assert_eq!(buf, data);
    }

    #[test]
    fn reader_retries_after_interrupted() {
        let reader = StdinReader::from_reader(InterruptedReader::new(b"after interrupt"));
        let mut buf = String::new();
        drain_until_eof(&reader, &mut buf);
        assert_eq!(buf, "after interrupt");
    }

    #[test]
    fn reader_error_sends_eof() {
        let reader = StdinReader::from_reader(ErrorReader);
        let mut buf = String::new();
        drain_until_eof(&reader, &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn reader_multiple_chunks_coalesce() {
        // Data larger than the 8192 internal buffer → multiple chunks
        let data = "y".repeat(8192 + 100);
        let reader = StdinReader::from_reader(Cursor::new(data.clone().into_bytes()));
        let mut buf = String::new();
        drain_until_eof(&reader, &mut buf);
        assert_eq!(buf.len(), 8292);
        assert_eq!(buf, data);
    }

    // --- drain_into direct channel tests ---

    #[test]
    fn drain_no_data_available() {
        let (tx, rx) = mpsc::channel::<StdinChunk>();
        let reader = StdinReader {
            rx,
            _handle: thread::spawn(|| {}),
        };
        // Nothing sent yet — tx kept alive so channel isn't disconnected
        let mut buf = String::new();
        let result = reader.drain_into(&mut buf);
        assert!(!result.got_data);
        assert!(!result.eof);
        assert!(buf.is_empty());
        drop(tx);
    }

    #[test]
    fn drain_coalesces_buffered_chunks() {
        let (tx, rx) = mpsc::channel();
        tx.send(StdinChunk::Data("aaa".into())).unwrap();
        tx.send(StdinChunk::Data("bbb".into())).unwrap();
        tx.send(StdinChunk::Eof).unwrap();
        drop(tx);

        let reader = StdinReader {
            rx,
            _handle: thread::spawn(|| {}),
        };
        let mut buf = String::new();
        let result = reader.drain_into(&mut buf);
        assert!(result.got_data);
        assert!(result.eof);
        assert_eq!(buf, "aaabbb");
    }
}
