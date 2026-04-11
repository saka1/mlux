//! Adaptive scroll policy — experimental.
//!
//! Consumes [`InputHistory`] to decide how much to scale the base
//! scroll step.  This module is intentionally the place where tuning
//! constants, heuristic thresholds, and other "ugly but necessary"
//! knobs live.  It can be freely rewritten without touching the
//! history infrastructure or the viewer loop.

use super::input_history::{InputHistory, ScrollDirection};

/// Scroll acceleration policy.
///
/// Currently stateless — all decisions are derived from the history
/// snapshot at call time.  Adding internal state (momentum, smoothing,
/// etc.) is fine; that's why this is a struct, not a bare function.
pub(super) struct ScrollPolicy {
    _private: (),
}

impl ScrollPolicy {
    pub(super) fn new() -> Self {
        Self { _private: () }
    }

    /// Return the effective scroll step (pixels) given the base step,
    /// current direction, and recent input history.
    ///
    /// The multiplier grows with the number of recent same-direction
    /// events, giving a "the longer you hold the key, the faster you
    /// go" feel.
    pub(super) fn effective_step(
        &self,
        base: u32,
        dir: ScrollDirection,
        history: &InputHistory,
    ) -> u32 {
        let count = history.recent_count(dir);

        // --- tuning zone (change freely) ---
        let multiplier: u32 = match count {
            0..=2 => 1,
            3..=5 => 2,
            6..=9 => 3,
            _ => 4,
        };

        base * multiplier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn history_with(dirs: &[ScrollDirection]) -> InputHistory {
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        for &d in dirs {
            h.record(d);
        }
        h
    }

    #[test]
    fn no_history_returns_base() {
        let policy = ScrollPolicy::new();
        let h = InputHistory::new(Duration::from_secs(5), 128);
        assert_eq!(policy.effective_step(72, ScrollDirection::Down, &h), 72);
    }

    #[test]
    fn low_count_no_acceleration() {
        let policy = ScrollPolicy::new();
        let h = history_with(&[ScrollDirection::Down, ScrollDirection::Down]);
        assert_eq!(policy.effective_step(72, ScrollDirection::Down, &h), 72);
    }

    #[test]
    fn mid_count_doubles() {
        let policy = ScrollPolicy::new();
        let h = history_with(&[ScrollDirection::Down; 4]);
        assert_eq!(policy.effective_step(72, ScrollDirection::Down, &h), 144);
    }

    #[test]
    fn high_count_triples() {
        let policy = ScrollPolicy::new();
        let h = history_with(&[ScrollDirection::Down; 8]);
        assert_eq!(policy.effective_step(72, ScrollDirection::Down, &h), 216);
    }

    #[test]
    fn very_high_count_quadruples() {
        let policy = ScrollPolicy::new();
        let h = history_with(&[ScrollDirection::Down; 15]);
        assert_eq!(policy.effective_step(72, ScrollDirection::Down, &h), 288);
    }

    #[test]
    fn opposite_direction_not_counted() {
        let policy = ScrollPolicy::new();
        let h = history_with(&[ScrollDirection::Up; 10]);
        // Asking for Down acceleration — Up events don't count
        assert_eq!(policy.effective_step(72, ScrollDirection::Down, &h), 72);
    }
}
