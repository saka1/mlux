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

pub(super) enum ScrollStrategy {
    /// Constant step: each keypress moves exactly `base` pixels.
    Fixed,
    /// Adaptive step: classifies cadence with [`ScrollPolicy`] using
    /// the shared history owned by `ScrollState`.
    Adaptive { policy: ScrollPolicy },
}

impl ScrollStrategy {
    pub(super) fn from_mode(mode: ScrollMode) -> Self {
        match mode {
            ScrollMode::Fixed => Self::Fixed,
            ScrollMode::Adaptive => Self::Adaptive {
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
    ///
    /// The push to `history` is the caller's responsibility — `step` is
    /// purely a read on the snapshot at call time.  See `+1` projection
    /// in `ScrollPolicy::classify`.
    pub(super) fn step(
        &self,
        scroll_step: u32,
        cell_h: u32,
        dir: ScrollDirection,
        history: &InputHistory,
    ) -> u32 {
        match self {
            Self::Fixed => scroll_step * cell_h,
            Self::Adaptive { policy } => policy.effective_step(cell_h, dir, history),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn empty_history() -> InputHistory {
        InputHistory::new(Duration::from_secs(5), 128)
    }

    #[test]
    fn fixed_is_scroll_step_times_cell_h() {
        let s = ScrollStrategy::from_mode(ScrollMode::Fixed);
        let h = empty_history();
        // 3 cells × 28 px/cell = 84.
        assert_eq!(s.step(3, 28, ScrollDirection::Down, &h), 84);
        // Fixed has no history effect — same result across calls.
        for _ in 0..10 {
            assert_eq!(s.step(3, 28, ScrollDirection::Down, &h), 84);
        }
    }

    #[test]
    fn adaptive_ignores_scroll_step() {
        // Adaptive uses its own internal base (2 cells × cell_h), so the
        // `scroll_step` argument is ignored — deliberately pass a value
        // that would be obviously wrong if it leaked through.
        let a = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        let b = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        let h = empty_history();
        assert_eq!(
            a.step(3, 24, ScrollDirection::Down, &h),
            b.step(999, 24, ScrollDirection::Down, &h),
        );
    }

    #[test]
    fn adaptive_accelerates_with_history() {
        let s = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        let mut h = empty_history();
        // cell_h=24 → adaptive base = 2 × 24 = 48.
        // First press (no priors): projection density=0+1=1 < 2 → Normal=48.
        assert_eq!(s.step(3, 24, ScrollDirection::Down, &h), 48);
        let _ = h.record(ScrollDirection::Down, 48);
        // Second press: 1 prior + projection = density=2 → Mid (×1.6 → 77).
        assert_eq!(s.step(3, 24, ScrollDirection::Down, &h), 77);
    }

    #[test]
    fn adaptive_isolates_directions() {
        let s = ScrollStrategy::from_mode(ScrollMode::Adaptive);
        let mut h = empty_history();
        // Fill history with Up events — should not accelerate Down.
        for _ in 0..10 {
            let _ = h.record(ScrollDirection::Up, -48);
        }
        // First Down press: 0 Down priors + projection=1 → Normal=48.
        assert_eq!(s.step(3, 24, ScrollDirection::Down, &h), 48);
    }
}
