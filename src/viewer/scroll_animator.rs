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

/// Duration of the ease-in ramp after a new scroll is initiated (ms).
///
/// Human smooth pursuit requires ~100ms to start tracking a moving target
/// (pursuit onset latency). During this window the effective half-life is
/// stretched so the animation starts slowly and ramps up, matching the eye.
const RAMP_DURATION_MS: f64 = 100.0;

/// Initial half-life scale factor at the start of the ramp.
///
/// Effective half-life begins at `DEFAULT_HALF_LIFE_MS × RAMP_INITIAL_SCALE`
/// (= 120ms) and smoothly decays to `DEFAULT_HALF_LIFE_MS` (40ms) over
/// `RAMP_DURATION_MS`. 120ms matches the recommended base value in the
/// design doc (§5.2).
const RAMP_INITIAL_SCALE: f64 = 3.0;

/// Natural frequency (rad/s) of the critically-damped spring animator.
///
/// `ω = 10` places peak velocity at `t = 1/ω = 100ms`, matching pursuit
/// onset latency (design doc §4.3). Larger ω feels snappier but reduces
/// the ease-in window; smaller ω feels softer but takes longer to settle.
const SPRING_OMEGA: f64 = 10.0;

/// Impulse gain: velocity kick (px/s) per pixel of incremental input.
///
/// The no-overshoot condition for a critically-damped spring is `v₀ ≤ ω·T`,
/// where `v₀ = impulse_px × gain` and `T ≈ impulse_px`. This simplifies to
/// `gain ≤ ω`. Setting `gain = ω` is the boundary: the spring reaches target
/// in the fastest monotone trajectory (`x(t) = T·(1 - e^(-ωt))`).
const SPRING_IMPULSE_GAIN: f64 = SPRING_OMEGA;

/// Velocity magnitude (px/s) below which a settled spring snaps.
///
/// Pairs with [`SNAP_THRESHOLD_PX`] — both conditions must hold for the
/// animator to report settled, so a fast-moving spring passing through
/// target doesn't prematurely stop. 5 px/s ≈ 0.1°/s at typical viewing
/// distance (design doc §4.8 minimum-perceptible-velocity).
const SPRING_SNAP_VELOCITY: f64 = 5.0;

/// Pluggable scroll interpolation.
///
/// Each variant owns the state its algorithm needs. Callers drive the
/// animator via [`Self::tick`] per frame and [`Self::set_target`] when
/// the desired position changes (some algorithms reset internal timers
/// on target change).
pub(super) enum ScrollAnimator {
    /// Exponential decay toward target with ease-in ramp on new scroll.
    ///
    /// Closed-form of `dx/dt = -λ(t)(x - target)` where `λ(t)` ramps from
    /// `ln(2) / (RAMP_INITIAL_SCALE × half_life_ms)` up to
    /// `ln(2) / half_life_ms` over `RAMP_DURATION_MS` via smoothstep.
    /// After the ramp the frame-rate-independence property holds
    /// (verified by `exp_decay_frame_rate_independent`).
    ExpDecay {
        current: f64,
        half_life_ms: f64,
        /// Elapsed ms since the last ease-in ramp was triggered.
        /// Initialised to `RAMP_DURATION_MS` (= fully ramped / no pending ease-in).
        ramp_elapsed_ms: f64,
    },

    /// Exponential decay with distance-adaptive half-life:
    /// `hl(d) = base × (1 + ln(1 + d/viewport))`.
    /// Near distances behave like ExpDecay(base); large jumps stretch
    /// sub-linearly (Stevens-power-law-consistent) so gg/G stays
    /// trackable by smooth pursuit (design doc §3.4, §4.7).
    ExpDecayAdaptive {
        current: f64,
        base_half_life_ms: f64,
    },

    /// Critically-damped spring with explicit velocity state.
    ///
    /// Solves the 2nd-order ODE `x'' + 2ω·x' + ω²·(x - target) = 0`
    /// with semi-implicit Euler. From rest, the step response
    /// `x(t) = T·(1 - (1 + ωt)·e^(-ωt))` has zero initial velocity,
    /// peak velocity at `t = 1/ω`, and no overshoot — ease-in is
    /// structural, not a separate ramp.
    ///
    /// Velocity persists across frames, so incremental `add_impulse`
    /// calls accumulate (Edge-style impulse model, design doc §2.2).
    /// Absolute `set_target` jumps are handled by the spring pull alone.
    DampedSpring {
        current: f64,
        velocity: f64,
        omega: f64,
        impulse_gain: f64,
    },
}

impl ScrollAnimator {
    /// Construct an ExpDecay animator starting at `initial` with the
    /// default half-life.
    pub(super) fn new_exp_decay(initial: f64) -> Self {
        Self::ExpDecay {
            current: initial,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
            ramp_elapsed_ms: RAMP_DURATION_MS,
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

    /// Construct a critically-damped spring animator at rest at `initial`.
    pub(super) fn new_damped_spring(initial: f64) -> Self {
        Self::DampedSpring {
            current: initial,
            velocity: 0.0,
            omega: SPRING_OMEGA,
            impulse_gain: SPRING_IMPULSE_GAIN,
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
            crate::config::ScrollAnimation::DampedSpring => Self::new_damped_spring(initial),
        }
    }

    /// Current interpolated position (sub-pixel precision).
    pub(super) fn current(&self) -> f64 {
        match self {
            Self::ExpDecay { current, .. }
            | Self::ExpDecayAdaptive { current, .. }
            | Self::DampedSpring { current, .. } => *current,
        }
    }

    /// Whether the animator has motion left toward `target`.
    /// Returns false once residual is below [`SNAP_THRESHOLD_PX`].
    /// `DampedSpring` also requires velocity below [`SPRING_SNAP_VELOCITY`]
    /// so fast-moving springs don't prematurely snap while passing target.
    pub(super) fn is_animating(&self, target: f64) -> bool {
        let close = (self.current() - target).abs() < SNAP_THRESHOLD_PX;
        match self {
            Self::DampedSpring { velocity, .. } => {
                !(close && velocity.abs() < SPRING_SNAP_VELOCITY)
            }
            _ => !close,
        }
    }

    /// Notify the animator that the scroll target has changed.
    ///
    /// `restart_ramp` should be `true` iff the animation was settled before
    /// this call (i.e. `!is_animating(old_target)`). When true, the ease-in
    /// ramp resets: the effective half-life starts at `RAMP_INITIAL_SCALE ×`
    /// base and smoothly decays to base over `RAMP_DURATION_MS`, compensating
    /// for pursuit-onset latency. When false (already scrolling), the ramp is
    /// left as-is — the eye is already tracking, so no ease-in is needed.
    pub(super) fn set_target(&mut self, restart_ramp: bool) {
        match self {
            Self::ExpDecay {
                ramp_elapsed_ms, ..
            } => {
                if restart_ramp {
                    *ramp_elapsed_ms = 0.0;
                }
            }
            Self::ExpDecayAdaptive { .. } => {}
            // DampedSpring reads `target` each tick and integrates velocity
            // toward it; no per-call state to update.
            Self::DampedSpring { .. } => {}
        }
    }

    /// Apply a velocity impulse to the animator, in units of pixels (the
    /// eventual extra displacement the impulse will produce if the spring
    /// is at rest and no further inputs arrive). For position-chase
    /// variants (`ExpDecay`, `ExpDecayAdaptive`) this is a no-op: they
    /// have no velocity state, so impulses are meaningless and the
    /// upstream layer's `target` accumulation is authoritative.
    pub(super) fn add_impulse(&mut self, delta_px: f64) {
        match self {
            Self::ExpDecay { .. } | Self::ExpDecayAdaptive { .. } => {}
            Self::DampedSpring {
                velocity,
                impulse_gain,
                ..
            } => {
                *velocity += delta_px * *impulse_gain;
            }
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
                ramp_elapsed_ms,
            } => {
                if dt.is_zero() {
                    return *current;
                }
                let dt_ms = dt.as_secs_f64() * 1000.0;
                // Smoothstep ramp: t ∈ [0,1] over RAMP_DURATION_MS.
                // hl_scale goes from RAMP_INITIAL_SCALE → 1.0, giving a
                // 120ms → 40ms effective half-life during the pursuit-onset window.
                let t = (*ramp_elapsed_ms / RAMP_DURATION_MS).min(1.0);
                let smooth = t * t * (3.0 - 2.0 * t);
                let hl_scale = RAMP_INITIAL_SCALE - (RAMP_INITIAL_SCALE - 1.0) * smooth;
                let effective_hl = *half_life_ms * hl_scale;
                *ramp_elapsed_ms = (*ramp_elapsed_ms + dt_ms).min(RAMP_DURATION_MS);
                let alpha = 1.0 - 0.5_f64.powf(dt_ms / effective_hl);
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
            Self::DampedSpring {
                current,
                velocity,
                omega,
                ..
            } => {
                if dt.is_zero() {
                    return *current;
                }
                // Semi-implicit (symplectic) Euler: update velocity first,
                // then integrate position with the new velocity. More stable
                // than explicit Euler for stiff springs and preserves the
                // critically-damped trajectory at typical frame rates.
                let dt_s = dt.as_secs_f64();
                let omega = *omega;
                let accel = omega * omega * (target - *current) - 2.0 * omega * *velocity;
                *velocity += accel * dt_s;
                *current += *velocity * dt_s;
                if (*current - target).abs() < SNAP_THRESHOLD_PX
                    && velocity.abs() < SPRING_SNAP_VELOCITY
                {
                    *current = target;
                    *velocity = 0.0;
                }
                *current
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
            ramp_elapsed_ms: RAMP_DURATION_MS,
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
            ramp_elapsed_ms: RAMP_DURATION_MS,
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
    fn set_target_does_not_change_position() {
        let mut a = ScrollAnimator::new_exp_decay(10.0);
        a.set_target(true);
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
            ramp_elapsed_ms: RAMP_DURATION_MS,
        };
        assert!(a.is_animating(100.0));
        // Residual < 0.5 → settled.
        let b = ScrollAnimator::ExpDecay {
            current: 99.6,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
            ramp_elapsed_ms: RAMP_DURATION_MS,
        };
        assert!(!b.is_animating(100.0));
    }

    // ---- ExpDecay ease-in ramp --------------------------------------------

    /// At t=0 the ramp makes effective hl = RAMP_INITIAL_SCALE × base.
    /// Progress after one base half-life must be less than 50% (the
    /// post-ramp steady-state value).
    #[test]
    fn exp_decay_ramp_starts_slow() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        a.set_target(true); // trigger ease-in
        let c = a.tick(
            100.0,
            NO_VP,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        // Steady-state (no ramp) would give ~50. Ramp must give less.
        assert!(c < 50.0, "expected ramp to slow start, got {c}");
    }

    /// After `RAMP_DURATION_MS` of accumulated ticks the ramp is complete;
    /// subsequent behaviour must match normal ExpDecay (hl_scale = 1).
    #[test]
    fn exp_decay_ramp_completes_after_100ms() {
        let ramp_ms = RAMP_DURATION_MS as u64;
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        a.set_target(true);
        // Burn through the ramp window with small steps.
        for _ in 0..10 {
            a.tick(100.0, NO_VP, Duration::from_millis(ramp_ms / 10));
        }
        // Now ramp should be complete. One more base half-life tick must
        // give ~50% of remaining residual, matching steady-state ExpDecay.
        let residual_before = 100.0 - a.current();
        let c = a.tick(
            100.0,
            NO_VP,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        let residual_after = 100.0 - c;
        assert!(
            (residual_after / residual_before - 0.5).abs() < 0.02,
            "expected residual to halve post-ramp, ratio={:.3}",
            residual_after / residual_before
        );
    }

    /// set_target(false) must not reset the ramp — simulates continuous
    /// keypress where the eye is already tracking.
    #[test]
    fn exp_decay_no_ramp_reset_when_already_animating() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        a.set_target(true); // trigger ramp
        // Advance partway through ramp.
        a.tick(100.0, NO_VP, Duration::from_millis(50));
        let elapsed_mid = match &a {
            ScrollAnimator::ExpDecay {
                ramp_elapsed_ms, ..
            } => *ramp_elapsed_ms,
            _ => panic!(),
        };
        // A second set_target(false) must not reset ramp_elapsed_ms.
        a.set_target(false);
        let elapsed_after = match &a {
            ScrollAnimator::ExpDecay {
                ramp_elapsed_ms, ..
            } => *ramp_elapsed_ms,
            _ => panic!(),
        };
        assert_eq!(elapsed_mid, elapsed_after, "ramp should not reset");
    }

    /// set_target(true) resets ramp_elapsed_ms to 0 so ease-in restarts.
    #[test]
    fn exp_decay_ramp_resets_on_new_scroll_from_settled() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        // Consume full ramp.
        a.tick(100.0, NO_VP, Duration::from_millis(200));
        // Now at rest (current → target). Trigger new ease-in.
        a.set_target(true);
        let elapsed = match &a {
            ScrollAnimator::ExpDecay {
                ramp_elapsed_ms, ..
            } => *ramp_elapsed_ms,
            _ => panic!(),
        };
        assert_eq!(elapsed, 0.0, "ramp_elapsed should reset to 0");
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

    // ---- DampedSpring -----------------------------------------------------

    /// Integrate a spring forward with fixed small steps. Returns the trace
    /// of (time_ms, current, velocity) so tests can inspect trajectories.
    fn spring_trace(
        mut a: ScrollAnimator,
        target: f64,
        steps: usize,
        dt_ms: f64,
    ) -> Vec<(f64, f64, f64)> {
        let dt = Duration::from_secs_f64(dt_ms / 1000.0);
        let mut out = Vec::with_capacity(steps + 1);
        out.push((0.0, a.current(), spring_velocity(&a)));
        for i in 1..=steps {
            a.tick(target, NO_VP, dt);
            out.push((i as f64 * dt_ms, a.current(), spring_velocity(&a)));
        }
        out
    }

    fn spring_velocity(a: &ScrollAnimator) -> f64 {
        match a {
            ScrollAnimator::DampedSpring { velocity, .. } => *velocity,
            _ => panic!("not a DampedSpring"),
        }
    }

    /// From rest, the first tick should move current by a sub-pixel amount
    /// (initial velocity is zero, so only one `dt²` worth of acceleration
    /// accumulates). This is the ease-in signature of critical damping.
    #[test]
    fn damped_spring_step_has_zero_initial_velocity() {
        let mut a = ScrollAnimator::new_damped_spring(0.0);
        // One 1ms tick against a target of 1000 px.
        a.tick(1000.0, NO_VP, Duration::from_millis(1));
        let c = a.current();
        // ω²·target·dt² = 100 · 1000 · 1e-6 = 0.1 px. Much smaller than
        // a pure-proportional model would give (which would jump ~100px).
        assert!(c < 1.0, "expected tiny initial step, got {c}");
    }

    /// Critically-damped spring must not overshoot target from rest.
    #[test]
    fn damped_spring_no_overshoot() {
        let a = ScrollAnimator::new_damped_spring(0.0);
        let trace = spring_trace(a, 100.0, 500, 2.0); // 1 second
        let max = trace
            .iter()
            .map(|(_, c, _)| *c)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(max <= 100.0 + 0.5, "overshoot detected: max={max}");
    }

    /// With gain = ω, a single impulse of N px into a target of N px must not
    /// overshoot. This is the boundary condition: v₀ = gain × N = ω × N = ω × T.
    #[test]
    fn damped_spring_no_overshoot_with_impulse() {
        let target = 48.0; // Normal scroll step at cell_h=24
        let mut a = ScrollAnimator::new_damped_spring(0.0);
        a.add_impulse(target);
        let trace = spring_trace(a, target, 500, 2.0);
        let max = trace
            .iter()
            .map(|(_, c, _)| *c)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(max <= target + 0.5, "overshoot with impulse: max={max}");
    }

    /// Peak velocity of a step response should occur near `t = 1/ω`.
    /// For ω=10 rad/s that's 100ms.
    #[test]
    fn damped_spring_peak_velocity_near_pursuit_onset() {
        let a = ScrollAnimator::new_damped_spring(0.0);
        let trace = spring_trace(a, 1000.0, 300, 1.0); // 300ms @ 1ms steps
        let (peak_t, _, _) = trace
            .iter()
            .max_by(|x, y| x.2.abs().partial_cmp(&y.2.abs()).unwrap())
            .copied()
            .unwrap();
        // Semi-implicit Euler puts the peak slightly before 1/ω = 100ms.
        // Accept a generous window around the analytic expectation.
        assert!(
            (70.0..=130.0).contains(&peak_t),
            "peak velocity at t={peak_t}ms, expected ≈100ms"
        );
    }

    /// Two rapid impulses must push the spring further than a single
    /// impulse — velocity state accumulates (Edge-style impulse model).
    /// Measured at peak displacement because critical damping returns
    /// cleanly to target without oscillation.
    #[test]
    fn damped_spring_impulses_accumulate() {
        // target == start so the spring pull opposes each impulse; peak
        // |current| reflects how much velocity was accumulated.
        fn peak_after_impulses(count: usize, gap_ms: u64) -> f64 {
            let mut a = ScrollAnimator::new_damped_spring(0.0);
            let mut peak = 0.0_f64;
            a.add_impulse(50.0);
            for i in 1..count {
                for _ in 0..(gap_ms / 2) {
                    a.tick(0.0, NO_VP, Duration::from_millis(2));
                    peak = peak.max(a.current().abs());
                }
                a.add_impulse(50.0);
                let _ = i;
            }
            for _ in 0..500 {
                a.tick(0.0, NO_VP, Duration::from_millis(2));
                peak = peak.max(a.current().abs());
            }
            peak
        }

        let peak_single = peak_after_impulses(1, 0);
        let peak_double = peak_after_impulses(2, 16);

        assert!(
            peak_double > peak_single * 1.3,
            "two impulses should peak higher than one: single={peak_single:.2}, double={peak_double:.2}"
        );
    }

    /// A long-running tick sequence must reach `is_animating == false`.
    #[test]
    fn damped_spring_settles() {
        let mut a = ScrollAnimator::new_damped_spring(0.0);
        for _ in 0..1000 {
            a.tick(100.0, NO_VP, Duration::from_millis(2));
        }
        assert!(!a.is_animating(100.0), "spring did not settle");
        assert!(
            (a.current() - 100.0).abs() < 1.0,
            "settled far from target: {}",
            a.current()
        );
    }

    /// `add_impulse` must be a no-op on position-chase animators so they
    /// behave identically whether mode_normal emits `ScrollTo` or `ScrollBy`.
    #[test]
    fn add_impulse_is_noop_for_exp_decay() {
        let mut a = ScrollAnimator::new_exp_decay(10.0);
        a.add_impulse(999.0);
        assert_eq!(a.current(), 10.0);
    }

    #[test]
    fn add_impulse_is_noop_for_exp_decay_adaptive() {
        let mut a = ScrollAnimator::new_exp_decay_adaptive(10.0);
        a.add_impulse(999.0);
        assert_eq!(a.current(), 10.0);
    }

    #[test]
    fn damped_spring_zero_dt_is_noop() {
        let mut a = ScrollAnimator::new_damped_spring(0.0);
        let c = a.tick(100.0, NO_VP, Duration::ZERO);
        assert_eq!(c, 0.0);
    }

    #[test]
    fn from_config_dispatches_damped_spring() {
        let a = ScrollAnimator::from_config(7.0, crate::config::ScrollAnimation::DampedSpring);
        assert!(matches!(
            a,
            ScrollAnimator::DampedSpring { current, velocity, .. }
                if current == 7.0 && velocity == 0.0
        ));
    }
}
