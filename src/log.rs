use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

const DEFAULT_CAPACITY: usize = 1024;

#[derive(Serialize, Deserialize)]
#[serde(remote = "log::Level")]
enum LevelSerde {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: SystemTime,
    #[serde(with = "LevelSerde")]
    pub level: log::Level,
    pub target: String,
    pub message: String,
}

impl LogEntry {
    /// Format as `[HH:MM:SS.mmm] [LEVEL] target: message`.
    ///
    /// Timestamps are UTC (no timezone dependency).
    pub fn format(&self) -> String {
        let dur = self
            .timestamp
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let total_secs = dur.as_secs();
        let hours = (total_secs / 3600) % 24;
        let minutes = (total_secs / 60) % 60;
        let seconds = total_secs % 60;
        let millis = dur.subsec_millis();
        format!(
            "[{hours:02}:{minutes:02}:{seconds:02}.{millis:03}] [{level}] {target}: {message}",
            level = self.level,
            target = self.target,
            message = self.message,
        )
    }
}

/// Thread-safe ring buffer of log entries.
#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Mutex<LogBufferInner>>,
}

struct LogBufferInner {
    entries: VecDeque<LogEntry>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogBufferInner {
                entries: VecDeque::with_capacity(capacity),
                capacity,
            })),
        }
    }

    pub fn push(&self, entry: LogEntry) {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.len() == inner.capacity {
            inner.entries.pop_front();
        }
        inner.entries.push_back(entry);
    }

    /// Returns a snapshot-clone of all entries.
    pub fn entries(&self) -> Vec<LogEntry> {
        let inner = self.inner.lock().unwrap();
        inner.entries.iter().cloned().collect()
    }

    /// Remove and return all entries, leaving the buffer empty.
    ///
    /// Tolerates a poisoned mutex so that logs can still be recovered
    /// after a panic (e.g. in a forked child process).
    pub fn drain(&self) -> Vec<LogEntry> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.entries.drain(..).collect()
    }
}

/// Custom logger: writes to ring buffer + optional file.
struct RingLog {
    buffer: LogBuffer,
    filter: env_filter::Filter,
    file: Option<Mutex<Box<dyn Write + Send>>>,
}

impl log::Log for RingLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.filter.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.filter.matches(record) {
            let entry = LogEntry {
                timestamp: SystemTime::now(),
                level: record.level(),
                target: record.target().to_string(),
                message: format!("{}", record.args()),
            };

            if let Some(ref file) = self.file
                && let Ok(mut f) = file.lock()
            {
                let _ = writeln!(f, "{}", entry.format());
            }

            self.buffer.push(entry);
        }
    }

    fn flush(&self) {
        if let Some(ref file) = self.file
            && let Ok(mut f) = file.lock()
        {
            let _ = f.flush();
        }
    }
}

/// Initialize the global logger. Returns the buffer handle.
pub fn init(debug: bool, log_file: Option<Box<dyn Write + Send>>) -> LogBuffer {
    let buffer = LogBuffer::new(DEFAULT_CAPACITY);

    let default_level = if debug { "debug" } else { "info" };
    let filter = env_filter::Builder::new()
        .parse(&std::env::var("RUST_LOG").unwrap_or_else(|_| default_level.to_string()))
        .build();

    let max_level = filter.filter();

    let logger = RingLog {
        buffer: buffer.clone(),
        filter,
        file: log_file.map(Mutex::new),
    };

    log::set_boxed_logger(Box::new(logger)).expect("logger already initialized");
    log::set_max_level(max_level);

    buffer
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    #[test]
    fn ring_buffer_capacity() {
        let buf = LogBuffer::new(3);
        let entries = buf.entries();
        assert!(entries.is_empty());

        buf.push(LogEntry {
            timestamp: SystemTime::now(),
            level: log::Level::Info,
            target: "t".into(),
            message: "a".into(),
        });
        buf.push(LogEntry {
            timestamp: SystemTime::now(),
            level: log::Level::Info,
            target: "t".into(),
            message: "b".into(),
        });
        buf.push(LogEntry {
            timestamp: SystemTime::now(),
            level: log::Level::Info,
            target: "t".into(),
            message: "c".into(),
        });
        buf.push(LogEntry {
            timestamp: SystemTime::now(),
            level: log::Level::Info,
            target: "t".into(),
            message: "d".into(),
        });
        let entries = buf.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "b"); // oldest dropped
        assert_eq!(entries[2].message, "d");
    }

    #[test]
    fn drain_clears_buffer() {
        let buf = LogBuffer::new(16);
        buf.push(LogEntry {
            timestamp: SystemTime::now(),
            level: log::Level::Info,
            target: "t".into(),
            message: "a".into(),
        });
        buf.push(LogEntry {
            timestamp: SystemTime::now(),
            level: log::Level::Warn,
            target: "t".into(),
            message: "b".into(),
        });
        let drained = buf.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message, "a");
        assert_eq!(drained[1].message, "b");
        assert!(buf.entries().is_empty());
    }

    #[test]
    fn log_entry_serde_roundtrip() {
        let original = LogEntry {
            timestamp: SystemTime::now(),
            level: log::Level::Warn,
            target: "mlux::test".into(),
            message: "hello".into(),
        };
        let encoded =
            bincode::serde::encode_to_vec(&original, bincode::config::standard()).unwrap();
        let (restored, _): (LogEntry, _) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
        assert_eq!(restored.level, log::Level::Warn);
        assert_eq!(restored.target, "mlux::test");
        assert_eq!(restored.message, "hello");
        let orig_ns = original
            .timestamp
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let rest_ns = restored
            .timestamp
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        assert_eq!(orig_ns, rest_ns);
    }

    #[test]
    fn format_entry_display() {
        let entry = LogEntry {
            timestamp: SystemTime::UNIX_EPOCH,
            level: log::Level::Warn,
            target: "mlux::viewer".into(),
            message: "test message".into(),
        };
        let formatted = entry.format();
        assert!(formatted.contains("WARN"));
        assert!(formatted.contains("mlux::viewer"));
        assert!(formatted.contains("test message"));
    }
}
