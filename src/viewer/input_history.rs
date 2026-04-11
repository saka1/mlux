//! User input history tracking.
//!
//! Records timestamped scroll events in a bounded ring buffer with
//! time-window pruning.  Provides query methods for downstream
//! consumers (e.g. scroll policy) — no policy decisions live here.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Scroll direction recorded per input event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScrollDirection {
    Down,
    Up,
}

struct InputRecord {
    direction: ScrollDirection,
    timestamp: Instant,
}

/// Bounded, time-windowed history of scroll input events.
///
/// Designed for event-driven recording (one call per key event) with
/// `&self`-only query methods so it can be shared without `&mut`.
pub(super) struct InputHistory {
    records: VecDeque<InputRecord>,
    window: Duration,
    max_entries: usize,
}

impl InputHistory {
    pub(super) fn new(window: Duration, max_entries: usize) -> Self {
        Self {
            records: VecDeque::with_capacity(max_entries.min(128)),
            window,
            max_entries,
        }
    }

    /// Record a scroll event. Lazily prunes stale entries.
    pub(super) fn record(&mut self, direction: ScrollDirection) {
        self.prune();
        if self.records.len() >= self.max_entries {
            self.records.pop_front();
        }
        self.records.push_back(InputRecord {
            direction,
            timestamp: Instant::now(),
        });
    }

    /// Number of recent events in the given direction within the window.
    pub(super) fn recent_count(&self, dir: ScrollDirection) -> usize {
        let cutoff = Instant::now() - self.window;
        self.records
            .iter()
            .rev()
            .take_while(|r| r.timestamp >= cutoff)
            .filter(|r| r.direction == dir)
            .count()
    }

    fn prune(&mut self) {
        let cutoff = Instant::now() - self.window;
        while self.records.front().is_some_and(|r| r.timestamp < cutoff) {
            self.records.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn empty_history() {
        let h = InputHistory::new(Duration::from_millis(500), 64);
        assert_eq!(h.recent_count(ScrollDirection::Down), 0);
        assert_eq!(h.recent_count(ScrollDirection::Up), 0);
    }

    #[test]
    fn record_and_count() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Up);
        assert_eq!(h.recent_count(ScrollDirection::Down), 2);
        assert_eq!(h.recent_count(ScrollDirection::Up), 1);
    }

    #[test]
    fn cap_limit_evicts_oldest() {
        let mut h = InputHistory::new(Duration::from_secs(60), 4);
        h.record(ScrollDirection::Up);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Down);
        // At cap; next record evicts the oldest Up
        h.record(ScrollDirection::Down);
        assert_eq!(h.records.len(), 4);
        assert_eq!(h.recent_count(ScrollDirection::Up), 0);
        assert_eq!(h.recent_count(ScrollDirection::Down), 4);
    }

    #[test]
    fn window_prune() {
        let mut h = InputHistory::new(Duration::from_millis(50), 128);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Down);
        assert_eq!(h.recent_count(ScrollDirection::Down), 2);

        // Wait for the window to expire
        thread::sleep(Duration::from_millis(60));

        // Old entries are outside the window — queries should not see them
        assert_eq!(h.recent_count(ScrollDirection::Down), 0);

        // New record triggers prune of stale entries
        h.record(ScrollDirection::Up);
        assert_eq!(h.records.len(), 1);
        assert_eq!(h.recent_count(ScrollDirection::Up), 1);
    }
}
