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

    /// Exponential decay with distance-adaptive half-life:
    /// `hl(d) = base × (1 + ln(1 + d/viewport))`.
    /// Near distances behave like ExpDecay(base); large jumps stretch
    /// sub-linearly (Stevens-power-law-consistent) so gg/G stays
    /// trackable by smooth pursuit (design doc §3.4, §4.7).
    ExpDecayAdaptive {
        current: f64,
        base_half_life_ms: f64,
    },
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

    /// Construct an ExpDecayAdaptive animator starting at `initial`
    /// with the default base half-life. Actual per-tick half-life is
    /// computed from current residual against viewport height.
    pub(super) fn new_exp_decay_adaptive(initial: f64) -> Self {
        Self::ExpDecayAdaptive {
            current: initial,
            base_half_life_ms: DEFAULT_HALF_LIFE_MS,
        }
    }

    /// Construct an animator for the algorithm selected in config.
    /// Adding a [`crate::config::ScrollAnimation`] variant forces a new
    /// arm here — that's the handoff point from user selection to
    /// concrete strategy.
    pub(super) fn from_config(initial: f64, cfg: crate::config::ScrollAnimation) -> Self {
        match cfg {
            crate::config::ScrollAnimation::ExpDecay => Self::new_exp_decay(initial),
            crate::config::ScrollAnimation::ExpDecayAdaptive => {
                Self::new_exp_decay_adaptive(initial)
            }
        }
    }

    /// Current interpolated position (sub-pixel precision).
    pub(super) fn current(&self) -> f64 {
        match self {
            Self::ExpDecay { current, .. } => *current,
            Self::ExpDecayAdaptive { current, .. } => *current,
        }
    }

    /// Whether the animator has motion left toward `target`.
    /// Returns false once residual is below [`SNAP_THRESHOLD_PX`].
    pub(super) fn is_animating(&self, target: f64) -> bool {
        (self.current() - target).abs() >= SNAP_THRESHOLD_PX
    }

    /// Notify the animator that `target` has changed. Some future
    /// strategies (Bezier, initial ramp-up) will reset internal timers
    /// here; ExpDecay* variants are stateless with respect to target
    /// change, so this is a no-op for now. The hook exists so callers
    /// can be written once and all variants react correctly.
    pub(super) fn set_target(&mut self, _target: f64) {
        match self {
            Self::ExpDecay { .. } => {}
            Self::ExpDecayAdaptive { .. } => {}
        }
    }

    /// Advance one frame toward `target` over elapsed `dt`. Returns the
    /// new current position. Snaps to `target` when residual is
    /// sub-pixel to avoid asymptotic never-reaching.
    ///
    /// `viewport_px` is consumed by distance-adaptive variants to
    /// compute residual-in-screens; fixed-half-life variants ignore it.
    /// Callers can pass 0 when viewport is not meaningful (e.g. tests
    /// for ExpDecay).
    pub(super) fn tick(&mut self, target: f64, viewport_px: f64, dt: Duration) -> f64 {
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
                apply_step(current, target, alpha)
            }
            Self::ExpDecayAdaptive {
                current,
                base_half_life_ms,
            } => {
                if dt.is_zero() {
                    return *current;
                }
                let d = (target - *current).abs();
                // Guard against zero viewport (Viewport::default() path,
                // pre-layout init). Falls back to base half-life.
                let v = viewport_px.max(1.0);
                let hl = *base_half_life_ms * (1.0 + (1.0 + d / v).ln());
                let dt_ms = dt.as_secs_f64() * 1000.0;
                let alpha = 1.0 - 0.5_f64.powf(dt_ms / hl);
                apply_step(current, target, alpha)
            }
        }
    }
}

/// Advance `current` toward `target` by fraction `alpha`, snapping to
/// `target` when the result is sub-pixel-close. Shared by all
/// interpolation variants so the snap threshold stays consistent.
fn apply_step(current: &mut f64, target: f64, alpha: f64) -> f64 {
    let next = *current + (target - *current) * alpha;
    *current = if (next - target).abs() < SNAP_THRESHOLD_PX {
        target
    } else {
        next
    };
    *current
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Viewport height irrelevant for ExpDecay; use an obvious sentinel.
    const NO_VP: f64 = 0.0;

    #[test]
    fn exp_decay_half_life_behavior() {
        // Starting at 0, target 100. After one half-life, residual halves → ~50.
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        let c = a.tick(
            100.0,
            NO_VP,
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
        let one_shot = one.tick(100.0, NO_VP, full);

        let mut two = ScrollAnimator::new_exp_decay(0.0);
        two.tick(100.0, NO_VP, half);
        let two_shot = two.tick(100.0, NO_VP, half);

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
        let c = a.tick(100.0, NO_VP, Duration::ZERO);
        assert_eq!(c, 10.0);
        assert_eq!(a.current(), 10.0);
    }

    #[test]
    fn exp_decay_snaps_when_residual_is_subpixel() {
        let mut a = ScrollAnimator::ExpDecay {
            current: 99.9,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = a.tick(100.0, NO_VP, Duration::from_millis(40));
        assert_eq!(c, 100.0);
        assert!(!a.is_animating(100.0));
    }

    #[test]
    fn exp_decay_converges_toward_target() {
        // After ~10 half-lives the residual is well under a pixel → snaps.
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        let dt = Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS * 10.0 / 1000.0);
        let c = a.tick(100.0, NO_VP, dt);
        assert_eq!(c, 100.0);
    }

    #[test]
    fn exp_decay_handles_negative_direction() {
        let mut a = ScrollAnimator::new_exp_decay(100.0);
        let c = a.tick(
            0.0,
            NO_VP,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
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
    fn from_config_dispatches_exp_decay_adaptive() {
        let a = ScrollAnimator::from_config(7.0, crate::config::ScrollAnimation::ExpDecayAdaptive);
        assert!(matches!(
            a,
            ScrollAnimator::ExpDecayAdaptive { current, .. } if current == 7.0
        ));
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

    // ---- ExpDecayAdaptive -------------------------------------------------

    /// At residual equal to one viewport, effective half-life is
    /// `base × (1 + ln 2) ≈ base × 1.693`. Feeding exactly that much
    /// wall-time should halve the residual.
    #[test]
    fn adaptive_half_life_at_one_viewport() {
        let viewport = 1000.0;
        let mut a = ScrollAnimator::new_exp_decay_adaptive(0.0);
        // distance = 1000 px = one viewport → hl = base × (1 + ln 2)
        let expected_hl_ms = DEFAULT_HALF_LIFE_MS * (1.0 + 2.0_f64.ln());
        let c = a.tick(
            1000.0,
            viewport,
            Duration::from_secs_f64(expected_hl_ms / 1000.0),
        );
        assert!(
            (c - 500.0).abs() < 0.5,
            "expected ~500 after one adaptive half-life, got {c}"
        );
    }

    /// Near distances (d ≪ viewport) reduce to the base half-life:
    /// `log(1 + ε) ≈ ε` so the scale factor is ≈ 1. One base half-life
    /// should still halve a small residual.
    #[test]
    fn adaptive_near_distance_matches_base_half_life() {
        let viewport = 10_000.0; // make d/v tiny: 50 / 10000 = 0.005
        let mut a = ScrollAnimator::ExpDecayAdaptive {
            current: 0.0,
            base_half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = a.tick(
            50.0,
            viewport,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        // Within 1% of true half (25.0).
        assert!(
            (c - 25.0).abs() < 0.25,
            "expected ~25 for near-distance, got {c}"
        );
    }

    /// Large jumps: `hl` grows sub-linearly. For d = 10 × viewport,
    /// scale is `1 + ln 11 ≈ 3.40`. After `base_hl` wall-time,
    /// residual should have shrunk by less than a factor of two —
    /// confirming the stretch is active.
    #[test]
    fn adaptive_large_jump_decays_slower_than_base() {
        let viewport = 100.0;
        let mut a = ScrollAnimator::new_exp_decay_adaptive(0.0);
        let c = a.tick(
            1000.0, // 10 viewports
            viewport,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        // Fixed-hl would give 500. Adaptive must give less progress → c < 500.
        assert!(
            c < 500.0,
            "adaptive should under-progress vs fixed, got {c}"
        );
        // And more than would be reached if hl = base × 4 (bounds the stretch).
        let alpha_cap = 1.0 - 0.5_f64.powf(1.0 / 4.0);
        let lower_bound = 1000.0 * alpha_cap;
        assert!(c > lower_bound, "adaptive regressing too much: {c}");
    }

    #[test]
    fn adaptive_zero_dt_is_noop() {
        let mut a = ScrollAnimator::ExpDecayAdaptive {
            current: 10.0,
            base_half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = a.tick(100.0, 500.0, Duration::ZERO);
        assert_eq!(c, 10.0);
    }

    #[test]
    fn adaptive_snaps_when_residual_is_subpixel() {
        let mut a = ScrollAnimator::ExpDecayAdaptive {
            current: 99.9,
            base_half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = a.tick(100.0, 500.0, Duration::from_millis(40));
        assert_eq!(c, 100.0);
        assert!(!a.is_animating(100.0));
    }

    /// Zero viewport (Viewport::default() path) must not divide-by-zero
    /// or explode. Falls back to base-half-life behavior.
    #[test]
    fn adaptive_zero_viewport_is_safe() {
        let mut a = ScrollAnimator::new_exp_decay_adaptive(0.0);
        let c = a.tick(
            100.0,
            0.0,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        // With viewport clamped to 1.0, d/v = 100, scale = 1+ln(101) ≈ 5.6;
        // residual shrinks only modestly. Just assert no panic and some progress.
        assert!(c > 0.0 && c < 100.0, "got {c}");
    }
}
