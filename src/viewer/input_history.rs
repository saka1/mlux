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

#[derive(Clone, Copy, Debug)]
pub(super) struct InputRecord {
    pub direction: ScrollDirection,
    pub delta_px: i32,
    pub timestamp: Instant,
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

    /// Record a scroll event. Returns the records that were evicted by
    /// this call (cap-based or time-based) so the caller can convolve
    /// their displacement into a permanent anchor before they are forgotten.
    pub(super) fn record(
        &mut self,
        direction: ScrollDirection,
        delta_px: i32,
    ) -> Vec<InputRecord> {
        let mut evicted = Vec::new();
        self.prune(&mut evicted);
        if self.records.len() >= self.max_entries
            && let Some(r) = self.records.pop_front()
        {
            evicted.push(r);
        }
        self.records.push_back(InputRecord {
            direction,
            delta_px,
            timestamp: Instant::now(),
        });
        evicted
    }

    /// Iterate all currently-buffered records (oldest first).
    /// Used by the animator to integrate the closed-form position formula.
    pub(super) fn iter(&self) -> impl Iterator<Item = &InputRecord> + '_ {
        self.records.iter()
    }

    /// Drain all buffered records (used on `ScrollAnchor` to flush before
    /// resetting the anchor).
    pub(super) fn drain(&mut self) -> Vec<InputRecord> {
        self.records.drain(..).collect()
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

    fn prune(&mut self, evicted: &mut Vec<InputRecord>) {
        let cutoff = Instant::now() - self.window;
        while let Some(r) = self.records.front()
            && r.timestamp < cutoff
        {
            evicted.push(self.records.pop_front().unwrap());
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
        let _ = h.record(ScrollDirection::Down, 0);
        let _ = h.record(ScrollDirection::Down, 0);
        let _ = h.record(ScrollDirection::Up, 0);
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
        let _ = h.record(ScrollDirection::Down, 0);
        thread::sleep(Duration::from_millis(60));
        let _ = h.record(ScrollDirection::Down, 0);
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
        let _ = h.record(ScrollDirection::Down, 0);
        thread::sleep(Duration::from_millis(40));
        let _ = h.record(ScrollDirection::Down, 0);
        let gap = h.last_gap(ScrollDirection::Down).unwrap();
        assert!(gap >= Duration::from_millis(40));
        assert!(gap < Duration::from_millis(500));
    }

    #[test]
    fn last_gap_none_with_fewer_than_two_same_direction() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        assert!(h.last_gap(ScrollDirection::Down).is_none());
        let _ = h.record(ScrollDirection::Down, 0);
        assert!(h.last_gap(ScrollDirection::Down).is_none());
        let _ = h.record(ScrollDirection::Up, 0);
        // Still only one Down event.
        assert!(h.last_gap(ScrollDirection::Down).is_none());
    }

    #[test]
    fn last_gap_ignores_interleaved_opposite_direction() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        let _ = h.record(ScrollDirection::Down, 0);
        thread::sleep(Duration::from_millis(30));
        let _ = h.record(ScrollDirection::Up, 0);
        thread::sleep(Duration::from_millis(30));
        let _ = h.record(ScrollDirection::Down, 0);
        // Gap between the two Down events includes the Up in the middle.
        let gap = h.last_gap(ScrollDirection::Down).unwrap();
        assert!(gap >= Duration::from_millis(60));
    }

    #[test]
    fn cap_limit_evicts_oldest() {
        let mut h = InputHistory::new(Duration::from_secs(60), 4);
        let _ = h.record(ScrollDirection::Up, 0);
        let _ = h.record(ScrollDirection::Down, 0);
        let _ = h.record(ScrollDirection::Down, 0);
        let _ = h.record(ScrollDirection::Down, 0);
        // At cap; next record evicts the oldest Up
        let _ = h.record(ScrollDirection::Down, 0);
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
        let _ = h.record(ScrollDirection::Down, 0);
        let _ = h.record(ScrollDirection::Down, 0);
        assert_eq!(h.records.len(), 2);

        // Wait past the outer window so the first two entries become stale.
        thread::sleep(Duration::from_millis(60));

        // The next record() triggers prune of stale entries.
        let _ = h.record(ScrollDirection::Up, 0);
        assert_eq!(h.records.len(), 1);
        assert_eq!(
            h.count_in_window(ScrollDirection::Up, Duration::from_secs(1)),
            1
        );
    }

    #[test]
    fn record_preserves_delta_px() {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        let _ = h.record(ScrollDirection::Down, 42);
        let _ = h.record(ScrollDirection::Up, -17);
        let v: Vec<_> = h.iter().map(|r| (r.direction, r.delta_px)).collect();
        assert_eq!(
            v,
            vec![
                (ScrollDirection::Down, 42),
                (ScrollDirection::Up, -17),
            ]
        );
    }

    #[test]
    fn record_returns_evicted_on_cap() {
        let mut h = InputHistory::new(Duration::from_secs(60), 2);
        let e1 = h.record(ScrollDirection::Down, 10);
        assert!(e1.is_empty());
        let e2 = h.record(ScrollDirection::Down, 20);
        assert!(e2.is_empty());
        let e3 = h.record(ScrollDirection::Down, 30);
        assert_eq!(e3.len(), 1);
        assert_eq!(e3[0].delta_px, 10);
    }

    #[test]
    fn record_returns_evicted_on_window_expiry() {
        let mut h = InputHistory::new(Duration::from_millis(50), 128);
        let _ = h.record(ScrollDirection::Down, 5);
        thread::sleep(Duration::from_millis(60));
        let evicted = h.record(ScrollDirection::Up, 7);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].delta_px, 5);
    }
}
