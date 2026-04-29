//! Scroll-step strategy dispatch.
//!
//! Selects between the `Fixed` (constant step) and `Adaptive`
//! (history-driven multiplier) behavior at runtime.  Each variant owns
//! its own state — `Fixed` is stateless, `Adaptive` owns the input
//! history buffer and the policy classifier.
//!
//! This is a closed sum type rather than `dyn Trait`: the set of
//! strategies is known at compile time, and enum-based dispatch gives
//! static `match` codegen with per-variant state ownership.  Adding a
//! new strategy means adding a variant and letting the compiler point
//! out every site that needs to handle it.

use super::input_history::{InputHistory, ScrollDirection};
use super::scroll_policy::ScrollPolicy;
use crate::config::ScrollMode;
use std::time::Duration;

/// Outer window for the adaptive history buffer.  Must be comfortably
/// larger than the policy's own sustain window (800ms).
const ADAPTIVE_HISTORY_WINDOW: Duration = Duration::from_millis(5000);

/// Hard cap on the adaptive history buffer.  Key repeat at 60Hz yields
/// ~300 events over the outer window; 128 is enough headroom without
/// unbounded growth.
const ADAPTIVE_HISTORY_CAP: usize = 128;

pub(super) enum ScrollStrategy {
    /// Constant step: each keypress moves exactly `base` pixels.
    Fixed,
    /// Adaptive step: records each event and defers to [`ScrollPolicy`].
    Adaptive {
        history: InputHistory,
        policy: ScrollPolicy,
    },
}

impl ScrollStrategy {
    pub(super) fn from_mode(mode: ScrollMode) -> Self {
        match mode {
            ScrollMode::Fixed => Self::Fixed,
            ScrollMode::Adaptive => Self::Adaptive {
                history: InputHistory::new(ADAPTIVE_HISTORY_WINDOW, ADAPTIVE_HISTORY_CAP),
                policy: ScrollPolicy::new(),
            },
        }
    }

    /// Compute the effective pixel step for a same-direction scroll event.
    ///
    /// `Fixed` multiplies the user's `scroll_step` by `cell_h`.  `Adaptive`
    /// ignores `scroll_step` entirely — it derives its own base from
    /// `cell_h` and internal constants (see [`ScrollPolicy`]), because the
    /// adaptive algorithm is self-tuning and its step size is not
    /// meaningfully a user preference.
    pub(super) fn step(&mut self, scroll_step: u32, cell_h: u32, dir: ScrollDirection) -> u32 {
        match self {
            Self::Fixed => scroll_step * cell_h,
            Self::Adaptive { history, policy } => {
                let _ = history.record(dir, 0);
                policy.effective_step(cell_h, dir, history)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_is_scroll_step_times_cell_h() {
        let mut s = ScrollStrategy::from_mode(ScrollMode::Fixed);
        // 3 cells × 28 px/cell = 84.
        assert_eq!(s.step(3, 28, ScrollDirection::Down), 84);
        // Fixed has no history effect — same result across calls.
        for _ in 0..10 {
            assert_eq!(s.step(3, 28, ScrollDirection::Down), 84);
        }
    }

    #[test]
    fn adaptive_ignores_scroll_step() {
        // Adaptive uses its own internal base (2 cells × cell_h), so the
        // `scroll_step` argument is ignored — deliberately pass a value
        // that would be obviously wrong if it leaked through.
        let mut a = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        let mut b = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        assert_eq!(
            a.step(3, 24, ScrollDirection::Down),
            b.step(999, 24, ScrollDirection::Down),
        );
    }

    #[test]
    fn adaptive_accelerates_with_history() {
        let mut s = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        // cell_h=24 → adaptive base = 2 × 24 = 48.
        // First press: density=1 < MID_THRESHOLD=2 → Normal (×1.0 → 48).
        assert_eq!(s.step(3, 24, ScrollDirection::Down), 48);
        // Second press: density=2 → Mid (×1.6 → 77).
        assert_eq!(s.step(3, 24, ScrollDirection::Down), 77);
    }

    #[test]
    fn adaptive_isolates_directions() {
        let mut s = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        // Fill history with Up events — should not accelerate Down.
        for _ in 0..10 {
            s.step(3, 24, ScrollDirection::Up);
        }
        // First Down press sees no prior Down gap → Normal (48).
        assert_eq!(s.step(3, 24, ScrollDirection::Down), 48);
    }
}
