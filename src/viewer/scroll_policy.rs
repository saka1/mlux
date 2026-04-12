//! Adaptive scroll policy — experimental.
//!
//! Consumes [`InputHistory`] to decide how much to scale the adaptive
//! base scroll step.  This module is intentionally the place where
//! tuning constants, heuristic thresholds, and other "ugly but
//! necessary" knobs live.  It can be freely rewritten without touching
//! the history infrastructure or the viewer loop.
//!
//! # Design
//!
//! A three-state classifier (`Normal` / `Mid` / `High`) is derived on
//! every call from the input history — no persistent state machine.
//!
//! - **Window-based classification only**: there is no explicit
//!   "decay gate" on `last_gap`.  Instead, the state is decided purely
//!   by how many events fall inside a few time windows.  When the user
//!   stops pressing, the oldest events age out of the density window
//!   on their own, and the count drops below `MID_THRESHOLD` → Normal.
//!   This avoids the boundary-flip problem that a single-threshold
//!   gate had at cadences near the gate width.
//! - **Precision recovery**: the Mid/Normal boundary is governed by
//!   `DENSITY_WINDOW`.  That width doubles as the user-facing
//!   "how long do I need to pause to snap back to Normal?" knob — if
//!   they want precise single-cell stepping, they wait longer than
//!   `DENSITY_WINDOW` between taps.
//! - **High hysteresis**: entering `High` still requires sustained
//!   input over both a short and a long window, so manual tapping
//!   can't reach it — `High` is effectively reserved for held-key
//!   repeat.  The multiplier there is deliberately modest (the OS
//!   repeat rate already produces many events per second).
//! - **Independent from `scroll_step`**: adaptive mode uses its own
//!   internal base of `ADAPTIVE_BASE_CELLS × cell_h` pixels per event.
//!   The user's `scroll_step` setting applies only to the `Fixed`
//!   strategy; adaptive is self-tuning and its step size is not a
//!   user preference.

use super::input_history::{InputHistory, ScrollDirection};
use log::debug;
use std::time::Duration;

/// Scroll acceleration policy.
///
/// Stateless — all decisions are derived from the history snapshot at
/// call time.
pub(super) struct ScrollPolicy;

// --- tuning zone (change freely) ---

/// Base step size for adaptive mode, expressed in terminal cells.
const ADAPTIVE_BASE_CELLS: u32 = 2;

/// Window governing the Normal ↔ Mid boundary.  Doubles as the
/// user-facing "pause this long to snap back to Normal" duration:
/// when the ring buffer has fewer than `MID_THRESHOLD` events inside
/// this window, the classifier returns Normal.  Tuned to feel snappy
/// when switching from Mid-speed paragraph scrolling to single-cell
/// precision — the shorter the window, the faster Normal is recovered.
const DENSITY_WINDOW: Duration = Duration::from_millis(300);

/// Wider window for the fast-count condition of `High`.  Must be ≥
/// `DENSITY_WINDOW` so that "burst" evidence isn't lost immediately
/// after the user eases off.
const HIGH_WINDOW: Duration = Duration::from_millis(600);

/// Longest window, requiring sustained density for `High`.
const SUSTAIN_WINDOW: Duration = Duration::from_millis(800);

const MID_THRESHOLD: usize = 2; // events within DENSITY_WINDOW
const HIGH_FAST_THRESHOLD: usize = 6; // events within HIGH_WINDOW
const HIGH_SUSTAIN_THRESHOLD: usize = 10; // events within SUSTAIN_WINDOW

// Per-state multipliers applied to the adaptive base (`ADAPTIVE_BASE_CELLS × cell_h`).
// Read directly: Normal is the base, Mid is 1.6×, High is 1.8×.
// High is only modestly above Mid on purpose: the OS key-repeat rate
// already multiplies total travel, so a large per-event coefficient
// on top produces runaway scrolling.
const MULTIPLIER_NORMAL: f32 = 1.0;
const MULTIPLIER_MID: f32 = 1.6;
const MULTIPLIER_HIGH: f32 = 1.8;

// --- end tuning zone ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollState {
    Normal,
    Mid,
    High,
}

impl ScrollState {
    fn multiplier(self) -> f32 {
        match self {
            Self::Normal => MULTIPLIER_NORMAL,
            Self::Mid => MULTIPLIER_MID,
            Self::High => MULTIPLIER_HIGH,
        }
    }
}

impl ScrollPolicy {
    pub(super) const fn new() -> Self {
        Self
    }

    /// Return the effective scroll step (pixels) for a same-direction
    /// event, given the current terminal cell height and recent input
    /// history.  The adaptive step is computed entirely from internal
    /// constants — `scroll_step` from the user's config is deliberately
    /// not consulted here.
    pub(super) fn effective_step(
        &self,
        cell_h: u32,
        dir: ScrollDirection,
        history: &InputHistory,
    ) -> u32 {
        let state = classify(history, dir);
        let base = ADAPTIVE_BASE_CELLS * cell_h;
        let effective = ((base as f32) * state.multiplier()).round() as u32;
        let density = history.count_in_window(dir, DENSITY_WINDOW);
        let fast = history.count_in_window(dir, HIGH_WINDOW);
        let sustain = history.count_in_window(dir, SUSTAIN_WINDOW);
        let gap_ms = history
            .last_gap(dir)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(-1);
        debug!(
            "scroll_policy: dir={dir:?} state={state:?} mult={:.2} cell_h={cell_h} base={base} eff={effective} density={density} fast={fast} sustain={sustain} last_gap_ms={gap_ms}",
            state.multiplier()
        );
        effective
    }
}

fn classify(history: &InputHistory, dir: ScrollDirection) -> ScrollState {
    // Normal is just "not enough recent activity".  No time-gap check
    // is needed: once the user pauses longer than DENSITY_WINDOW, the
    // old events fall outside the window and density naturally drops
    // below MID_THRESHOLD.  This replaces an earlier explicit
    // `last_gap > DECAY_GATE` check that caused Mid↔Normal oscillation
    // at cadences near the gate width (e.g. 300-400ms tapping with a
    // 300ms gate would flip state on every other event).
    let density = history.count_in_window(dir, DENSITY_WINDOW);
    if density < MID_THRESHOLD {
        return ScrollState::Normal;
    }

    let fast = history.count_in_window(dir, HIGH_WINDOW);
    let sustain = history.count_in_window(dir, SUSTAIN_WINDOW);
    if fast >= HIGH_FAST_THRESHOLD && sustain >= HIGH_SUSTAIN_THRESHOLD {
        ScrollState::High
    } else {
        ScrollState::Mid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    // With ADAPTIVE_BASE_CELLS=2 and cell_h=24 → base=48.
    // Multipliers 1.0 / 1.6 / 1.8 → Normal=48, Mid≈77, High≈86.
    // (Exact: 48×1.6=76.8 → rounds to 77; 48×1.8=86.4 → rounds to 86.)
    const CELL_H: u32 = 24;

    #[test]
    fn empty_history_is_normal() {
        let policy = ScrollPolicy::new();
        let h = InputHistory::new(Duration::from_secs(5), 128);
        assert_eq!(policy.effective_step(CELL_H, ScrollDirection::Down, &h), 48);
    }

    #[test]
    fn single_press_is_normal() {
        // 1 event in DENSITY_WINDOW < MID_THRESHOLD=2 → Normal.
        let policy = ScrollPolicy::new();
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        h.record(ScrollDirection::Down);
        assert_eq!(policy.effective_step(CELL_H, ScrollDirection::Down, &h), 48);
    }

    #[test]
    fn two_rapid_events_enter_mid() {
        // 2 events within DENSITY_WINDOW → reaches MID_THRESHOLD → Mid.
        let policy = ScrollPolicy::new();
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        h.record(ScrollDirection::Down);
        h.record(ScrollDirection::Down);
        assert_eq!(policy.effective_step(CELL_H, ScrollDirection::Down, &h), 77);
    }

    #[test]
    fn high_requires_both_fast_and_sustain() {
        // 12 rapid events — density ≥ 2, fast ≥ 6, sustain ≥ 10 → High.
        let policy = ScrollPolicy::new();
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        for _ in 0..12 {
            h.record(ScrollDirection::Down);
        }
        assert_eq!(policy.effective_step(CELL_H, ScrollDirection::Down, &h), 86);
    }

    #[test]
    fn high_fast_but_not_sustained_stays_mid() {
        // 6 events: fast = 6 (≥ HIGH_FAST_THRESHOLD) but sustain = 6
        // (< HIGH_SUSTAIN_THRESHOLD = 10) → Mid.  SUSTAIN window
        // (800ms) ⊇ HIGH window (600ms), so any fast event is also a
        // sustain event — the sustain count can't exceed the event
        // count.
        let policy = ScrollPolicy::new();
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        for _ in 0..6 {
            h.record(ScrollDirection::Down);
        }
        assert_eq!(policy.effective_step(CELL_H, ScrollDirection::Down, &h), 77);
    }

    #[test]
    fn idle_past_density_window_returns_to_normal() {
        // After a burst of High-qualifying input, pause longer than
        // DENSITY_WINDOW.  Old events age out → density < MID_THRESHOLD
        // → Normal.  This is the stateless replacement for the old
        // explicit decay gate: precision recovery falls out of the
        // window mechanics.
        let policy = ScrollPolicy::new();
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        for _ in 0..12 {
            h.record(ScrollDirection::Down);
        }
        thread::sleep(DENSITY_WINDOW + Duration::from_millis(50));
        h.record(ScrollDirection::Down);
        // Only this single post-idle event is inside DENSITY_WINDOW →
        // density=1 < 2 → Normal.
        assert_eq!(policy.effective_step(CELL_H, ScrollDirection::Down, &h), 48);
    }

    #[test]
    fn opposite_direction_not_counted() {
        let policy = ScrollPolicy::new();
        let mut h = InputHistory::new(Duration::from_secs(5), 128);
        for _ in 0..12 {
            h.record(ScrollDirection::Up);
        }
        // Asking for Down — all Up events are ignored → density=0 → Normal.
        assert_eq!(policy.effective_step(CELL_H, ScrollDirection::Down, &h), 48);
    }

    #[test]
    fn scales_with_cell_height() {
        let policy = ScrollPolicy::new();
        let h = InputHistory::new(Duration::from_secs(5), 128);
        // cell_h=30 → base=60, Normal multiplier → 60.
        assert_eq!(policy.effective_step(30, ScrollDirection::Down, &h), 60);
    }
}
