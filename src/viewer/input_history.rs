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

#[derive(Debug)]
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

    /// Count same-direction events within the given sub-window
    /// (measured from `now` backwards).  The sub-window is typically
    /// much smaller than `self.window`.
    pub(super) fn count_in_window(&self, dir: ScrollDirection, window: Duration) -> usize {
        let cutoff = Instant::now() - window;
        self.records
            .iter()
            .rev()
            .take_while(|r| r.timestamp >= cutoff)
            .filter(|r| r.direction == dir)
            .count()
    }

    /// Gap between the two most recent same-direction events.
    ///
    /// This is the "inter-event interval" for the *previous* same-direction
    /// press — useful for decay-gate style logic where you want to know
    /// "was the user already pressing this direction, or did they just
    /// start?"  Returns `None` if fewer than two such events exist.
    ///
    /// Unlike [`Self::time_since_last`], this is meaningful to call *after*
    /// recording the current event: the "last" record is the current event
    /// (dt ≈ 0), but the gap between the last two tells you the real
    /// inter-press interval.
    pub(super) fn last_gap(&self, dir: ScrollDirection) -> Option<Duration> {
        let mut same_dir = self.records.iter().rev().filter(|r| r.direction == dir);
        let newest = same_dir.next()?;
        let previous = same_dir.next()?;
        Some(
            newest
                .timestamp
                .saturating_duration_since(previous.timestamp),
        )
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
        assert_eq!(
            h.count_in_window(ScrollDirection::Down, Duration::from_secs(1)),
            0
        );
        assert_eq!(h.last_gap(ScrollDirection::Down), None);
    }

    #[test]
    fn count_in_window_filters_by_direction() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Up);
        assert_eq!(
            h.count_in_window(ScrollDirection::Down, Duration::from_secs(1)),
            2
        );
        assert_eq!(
            h.count_in_window(ScrollDirection::Up, Duration::from_secs(1)),
            1
        );
    }

    #[test]
    fn count_in_window_respects_sub_window() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        h.record(ScrollDirection::Down);
        thread::sleep(Duration::from_millis(60));
        h.record(ScrollDirection::Down);
        // Both events live in the 5s outer buffer, but only one falls
        // within a 30ms sub-window.
        assert_eq!(
            h.count_in_window(ScrollDirection::Down, Duration::from_millis(30)),
            1
        );
        assert_eq!(
            h.count_in_window(ScrollDirection::Down, Duration::from_millis(200)),
            2
        );
    }

    #[test]
    fn last_gap_returns_interval_between_two_newest() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        h.record(ScrollDirection::Down);
        thread::sleep(Duration::from_millis(40));
        h.record(ScrollDirection::Down);
        let gap = h.last_gap(ScrollDirection::Down).unwrap();
        assert!(gap >= Duration::from_millis(40));
        assert!(gap < Duration::from_millis(500));
    }

    #[test]
    fn last_gap_none_with_fewer_than_two_same_direction() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        assert!(h.last_gap(ScrollDirection::Down).is_none());
        h.record(ScrollDirection::Down);
        assert!(h.last_gap(ScrollDirection::Down).is_none());
        h.record(ScrollDirection::Up);
        // Still only one Down event.
        assert!(h.last_gap(ScrollDirection::Down).is_none());
    }

    #[test]
    fn last_gap_ignores_interleaved_opposite_direction() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        h.record(ScrollDirection::Down);
        thread::sleep(Duration::from_millis(30));
        h.record(ScrollDirection::Up);
        thread::sleep(Duration::from_millis(30));
        h.record(ScrollDirection::Down);
        // Gap between the two Down events includes the Up in the middle.
        let gap = h.last_gap(ScrollDirection::Down).unwrap();
        assert!(gap >= Duration::from_millis(60));
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
        assert_eq!(
            h.count_in_window(ScrollDirection::Up, Duration::from_secs(60)),
            0
        );
        assert_eq!(
            h.count_in_window(ScrollDirection::Down, Duration::from_secs(60)),
            4
        );
    }

    #[test]
    fn outer_window_prune_on_record() {
        let mut h = InputHistory::new(Duration::from_millis(50), 128);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Down);
        assert_eq!(h.records.len(), 2);

        // Wait past the outer window so the first two entries become stale.
        thread::sleep(Duration::from_millis(60));

        // The next record() triggers prune of stale entries.
        h.record(ScrollDirection::Up);
        assert_eq!(h.records.len(), 1);
        assert_eq!(
            h.count_in_window(ScrollDirection::Up, Duration::from_secs(1)),
            1
        );
    }
}
