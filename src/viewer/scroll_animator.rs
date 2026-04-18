//! Scroll position interpolation strategies.
//!
//! Time-evolution of `current → target` is its own domain: half-life, snap
//! thresholds, per-algorithm state (velocity, elapsed time, etc.). It is
//! deliberately kept out of `layout.rs` (static geometry + position
//! snapshot) and `display_state.rs` (KGP plumbing) — this module speaks
//! only in scalars, durations, and algorithm parameters.
//!
//! # Pluggability
//!
//! `ScrollAnimator` is a closed enum rather than `dyn Trait`, matching the
//! convention in `scroll.rs` for `ScrollStrategy`. Adding a new algorithm
//! means adding a variant; the compiler then points out every `match` that
//! needs to handle it.

use std::time::Duration;

/// Default half-life for the ExpDecay algorithm (ms).
///
/// The residual distance to target halves every `DEFAULT_HALF_LIFE_MS`,
/// regardless of frame rate. Tuned empirically — see
/// `docs/2026-04-18-experiment-subcell-scroll.md` §4.
pub(super) const DEFAULT_HALF_LIFE_MS: f64 = 40.0;

/// Pixel residual below which the animator snaps `current` to `target`
/// and `is_animating` reports settled.
const SNAP_THRESHOLD_PX: f64 = 0.5;

/// Pluggable scroll interpolation.
///
/// Each variant owns the state its algorithm needs. Callers drive the
/// animator via [`Self::tick`] per frame and [`Self::set_target`] when
/// the desired position changes (some algorithms reset internal timers
/// on target change).
pub(super) enum ScrollAnimator {
    /// Exponential decay toward target. Closed-form of
    /// `dx/dt = -λ (x - target)` with `λ = ln(2) / half_life_ms`.
    /// Two half-`dt` steps compose to the same result as one full-`dt`
    /// step (verified by `exp_decay_frame_rate_independent`).
    ExpDecay { current: f64, half_life_ms: f64 },
}

impl ScrollAnimator {
    /// Construct an ExpDecay animator starting at `initial` with the
    /// default half-life.
    pub(super) fn new_exp_decay(initial: f64) -> Self {
        Self::ExpDecay {
            current: initial,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
        }
    }

    /// Construct an animator for the algorithm selected in config.
    /// Adding a [`crate::config::ScrollAnimation`] variant forces a new
    /// arm here — that's the handoff point from user selection to
    /// concrete strategy.
    pub(super) fn from_config(initial: f64, cfg: crate::config::ScrollAnimation) -> Self {
        match cfg {
            crate::config::ScrollAnimation::ExpDecay => Self::new_exp_decay(initial),
        }
    }

    /// Current interpolated position (sub-pixel precision).
    pub(super) fn current(&self) -> f64 {
        match self {
            Self::ExpDecay { current, .. } => *current,
        }
    }

    /// Whether the animator has motion left toward `target`.
    /// Returns false once residual is below [`SNAP_THRESHOLD_PX`].
    pub(super) fn is_animating(&self, target: f64) -> bool {
        (self.current() - target).abs() >= SNAP_THRESHOLD_PX
    }

    /// Notify the animator that `target` has changed. Some future
    /// strategies (Bezier, initial ramp-up) will reset internal timers
    /// here; ExpDecay is stateless with respect to target change, so
    /// this is a no-op for now. The hook exists so callers can be
    /// written once and all variants react correctly.
    pub(super) fn set_target(&mut self, _target: f64) {
        match self {
            Self::ExpDecay { .. } => {}
        }
    }

    /// Advance one frame toward `target` over elapsed `dt`. Returns the
    /// new current position. Snaps to `target` when residual is
    /// sub-pixel to avoid asymptotic never-reaching.
    pub(super) fn tick(&mut self, target: f64, dt: Duration) -> f64 {
        match self {
            Self::ExpDecay {
                current,
                half_life_ms,
            } => {
                if dt.is_zero() {
                    return *current;
                }
                let dt_ms = dt.as_secs_f64() * 1000.0;
                let alpha = 1.0 - 0.5_f64.powf(dt_ms / *half_life_ms);
                let next = *current + (target - *current) * alpha;
                *current = if (next - target).abs() < SNAP_THRESHOLD_PX {
                    target
                } else {
                    next
                };
                *current
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_decay_half_life_behavior() {
        // Starting at 0, target 100. After one half-life, residual halves → ~50.
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        let c = a.tick(
            100.0,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        assert!((c - 50.0).abs() < 0.01, "expected ~50, got {c}");
    }

    #[test]
    fn exp_decay_frame_rate_independent() {
        // Two half-dt steps must equal one full-dt step (within FP error).
        let full = Duration::from_millis(40);
        let half = Duration::from_millis(20);

        let mut one = ScrollAnimator::new_exp_decay(0.0);
        let one_shot = one.tick(100.0, full);

        let mut two = ScrollAnimator::new_exp_decay(0.0);
        two.tick(100.0, half);
        let two_shot = two.tick(100.0, half);

        assert!(
            (one_shot - two_shot).abs() < 1e-9,
            "one_shot={one_shot} two_shot={two_shot}"
        );
    }

    #[test]
    fn exp_decay_zero_dt_is_noop() {
        let mut a = ScrollAnimator::ExpDecay {
            current: 10.0,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = a.tick(100.0, Duration::ZERO);
        assert_eq!(c, 10.0);
        assert_eq!(a.current(), 10.0);
    }

    #[test]
    fn exp_decay_snaps_when_residual_is_subpixel() {
        let mut a = ScrollAnimator::ExpDecay {
            current: 99.9,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = a.tick(100.0, Duration::from_millis(40));
        assert_eq!(c, 100.0);
        assert!(!a.is_animating(100.0));
    }

    #[test]
    fn exp_decay_converges_toward_target() {
        // After ~10 half-lives the residual is well under a pixel → snaps.
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        let dt = Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS * 10.0 / 1000.0);
        let c = a.tick(100.0, dt);
        assert_eq!(c, 100.0);
    }

    #[test]
    fn exp_decay_handles_negative_direction() {
        let mut a = ScrollAnimator::new_exp_decay(100.0);
        let c = a.tick(0.0, Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0));
        assert!((c - 50.0).abs() < 0.01, "expected ~50, got {c}");
    }

    #[test]
    fn set_target_is_noop_for_exp_decay() {
        let mut a = ScrollAnimator::new_exp_decay(10.0);
        a.set_target(500.0);
        // No observable change to internal state for ExpDecay — it just
        // reads `target` each tick. Future variants will reset timers here.
        assert_eq!(a.current(), 10.0);
    }

    #[test]
    fn from_config_dispatches_exp_decay() {
        let a = ScrollAnimator::from_config(7.0, crate::config::ScrollAnimation::ExpDecay);
        assert!(matches!(a, ScrollAnimator::ExpDecay { current, .. } if current == 7.0));
    }

    #[test]
    fn is_animating_threshold() {
        // Residual >= 0.5 → still animating.
        let a = ScrollAnimator::ExpDecay {
            current: 99.4,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        assert!(a.is_animating(100.0));
        // Residual < 0.5 → settled.
        let b = ScrollAnimator::ExpDecay {
            current: 99.6,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        assert!(!b.is_animating(100.0));
    }
}
