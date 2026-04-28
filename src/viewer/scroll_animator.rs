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

/// Friction time constant (ms) of the Kinetic animator.
///
/// Velocity decays as `v(t) = v₀·e^(-t/τ)`, position evolves
/// `x(t) = x₀ + v₀·τ·(1 - e^(-t/τ))`. Settle time is roughly `5τ`
/// (≈ 250ms for τ=50). Smaller τ feels snappier and settles faster
/// but reduces the "glide" momentum sensation; larger τ extends the
/// kinetic feel but lengthens the tail.
const DEFAULT_KINETIC_TAU_MS: f64 = 50.0;

/// Velocity magnitude (px/s) below which the Kinetic animator snaps.
///
/// Pairs with [`SNAP_THRESHOLD_PX`] — both conditions must hold so a
/// fast-moving glide passing target doesn't prematurely stop. 30 px/s
/// = ~1 px / 33 ms (≈ 30 fps), the boundary above which sub-pixel
/// motion remains perceptually continuous; below it, integer rounding
/// produces the visible "ticking creep" the kinetic snap is meant to
/// eliminate.
const KINETIC_SNAP_VELOCITY: f64 = 30.0;

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

    /// Kinetic (iOS-style momentum scroll) — velocity-driven with
    /// pure friction decay.
    ///
    /// Solves the 1st-order ODE `dv/dt = -v/τ` exactly per frame:
    /// `v(t+dt) = v(t)·e^(-dt/τ)`,
    /// `x(t+dt) = x(t) + v(t)·τ·(1 - e^(-dt/τ))`.
    /// No restoring force toward target — landing position is
    /// `x_∞ = x₀ + v·τ`, set implicitly by velocity.
    ///
    /// Two velocity injection paths preserve the invariant
    /// `target == current + velocity·τ`:
    /// - [`Self::add_impulse`] for incremental scroll (j/k): adds
    ///   `delta_px / τ`, so impulses stack as momentum.
    /// - [`Self::set_landing`] for absolute jumps (gg/G/search):
    ///   replaces velocity with `(target - current) / τ`, overriding
    ///   any prior momentum (matches iOS "scroll-to-top" semantics).
    Kinetic {
        current: f64,
        velocity: f64,
        tau_ms: f64,
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

    /// Construct a Kinetic animator at rest at `initial`.
    pub(super) fn new_kinetic(initial: f64) -> Self {
        Self::Kinetic {
            current: initial,
            velocity: 0.0,
            tau_ms: DEFAULT_KINETIC_TAU_MS,
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
            crate::config::ScrollAnimation::Kinetic => Self::new_kinetic(initial),
        }
    }

    /// Current interpolated position (sub-pixel precision).
    pub(super) fn current(&self) -> f64 {
        match self {
            Self::ExpDecay { current, .. }
            | Self::ExpDecayAdaptive { current, .. }
            | Self::Kinetic { current, .. } => *current,
        }
    }

    /// Whether the animator has motion left toward `target`.
    /// Returns false once residual is below [`SNAP_THRESHOLD_PX`].
    /// `Kinetic` also requires velocity below [`KINETIC_SNAP_VELOCITY`]
    /// so a fast glide passing target doesn't prematurely snap.
    pub(super) fn is_animating(&self, target: f64) -> bool {
        let close = (self.current() - target).abs() < SNAP_THRESHOLD_PX;
        match self {
            Self::Kinetic { velocity, .. } => !(close && velocity.abs() < KINETIC_SNAP_VELOCITY),
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
            // Kinetic responds via add_impulse / set_landing instead;
            // bare target-change notification carries no useful signal.
            Self::Kinetic { .. } => {}
        }
    }

    /// Apply a velocity impulse, in units of pixels of asymptotic
    /// displacement (the extra distance the impulse glides to from
    /// rest if no further inputs arrive). Used for incremental scroll
    /// (j/k) so rapid keypresses accumulate momentum.
    ///
    /// No-op for position-chase variants (`ExpDecay`, `ExpDecayAdaptive`):
    /// they have no velocity state, so the upstream layer's `target`
    /// accumulation is authoritative.
    pub(super) fn add_impulse(&mut self, delta_px: f64) {
        match self {
            Self::ExpDecay { .. } | Self::ExpDecayAdaptive { .. } => {}
            Self::Kinetic {
                velocity, tau_ms, ..
            } => {
                let tau_s = *tau_ms / 1000.0;
                *velocity += delta_px / tau_s;
            }
        }
    }

    /// Override velocity so the kinetic glide settles at `target_px`
    /// asymptotically. Used for absolute scroll jumps (gg/G/search):
    /// any pre-existing momentum is discarded — matches iOS-style
    /// "scroll-to-top" semantics, where tapping the destination cancels
    /// in-flight inertia. No-op for position-chase variants.
    pub(super) fn set_landing(&mut self, target_px: f64) {
        match self {
            Self::ExpDecay { .. } | Self::ExpDecayAdaptive { .. } => {}
            Self::Kinetic {
                current,
                velocity,
                tau_ms,
            } => {
                let tau_s = *tau_ms / 1000.0;
                *velocity = (target_px - *current) / tau_s;
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
            Self::Kinetic {
                current,
                velocity,
                tau_ms,
            } => {
                if dt.is_zero() {
                    return *current;
                }
                // Closed-form integration of dv/dt = -v/τ over `dt`:
                // exact, frame-rate independent, stable for any dt.
                let dt_s = dt.as_secs_f64();
                let tau_s = *tau_ms / 1000.0;
                let decay = (-dt_s / tau_s).exp();
                *current += *velocity * tau_s * (1.0 - decay);
                *velocity *= decay;
                if (*current - target).abs() < SNAP_THRESHOLD_PX
                    && velocity.abs() < KINETIC_SNAP_VELOCITY
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

    // ---- Kinetic ----------------------------------------------------------

    fn kinetic_velocity(a: &ScrollAnimator) -> f64 {
        match a {
            ScrollAnimator::Kinetic { velocity, .. } => *velocity,
            _ => panic!("not a Kinetic"),
        }
    }

    /// `add_impulse(d)` from rest must set velocity such that the
    /// asymptotic glide lands `d` px away (`x_∞ = x₀ + v·τ`, so v = d/τ).
    #[test]
    fn kinetic_add_impulse_glides_to_landing() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        a.add_impulse(72.0);
        // Run long enough for residual to vanish.
        for _ in 0..200 {
            a.tick(72.0, NO_VP, Duration::from_millis(2));
        }
        assert!(
            (a.current() - 72.0).abs() < 0.5,
            "kinetic should land at 72, got {}",
            a.current()
        );
    }

    /// Velocity decays exponentially with time constant τ. After one τ
    /// elapsed, |v| should be v₀/e.
    #[test]
    fn kinetic_velocity_decays_with_tau() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        a.add_impulse(1000.0); // v₀ = 1000 / (50/1000) = 20000 px/s
        let v0 = kinetic_velocity(&a);
        // One τ = 50 ms.
        a.tick(
            f64::INFINITY, // target irrelevant — kinetic doesn't pull toward it
            NO_VP,
            Duration::from_secs_f64(DEFAULT_KINETIC_TAU_MS / 1000.0),
        );
        let v1 = kinetic_velocity(&a);
        let ratio = v1 / v0;
        let expected = (-1.0_f64).exp(); // 1/e ≈ 0.368
        assert!(
            (ratio - expected).abs() < 0.01,
            "after τ velocity should be v₀/e ≈ {expected}, got ratio={ratio}"
        );
    }

    /// Frame-rate independence: two half-dt steps must equal one full-dt
    /// step (within FP error). Closed-form integration guarantees this
    /// exactly, not just for small dt as in Euler.
    #[test]
    fn kinetic_frame_rate_independent() {
        let full = Duration::from_millis(20);
        let half = Duration::from_millis(10);

        let mut one = ScrollAnimator::new_kinetic(0.0);
        one.add_impulse(100.0);
        one.tick(100.0, NO_VP, full);

        let mut two = ScrollAnimator::new_kinetic(0.0);
        two.add_impulse(100.0);
        two.tick(100.0, NO_VP, half);
        two.tick(100.0, NO_VP, half);

        assert!(
            (one.current() - two.current()).abs() < 1e-9,
            "one_step={} two_step={}",
            one.current(),
            two.current()
        );
    }

    /// Two rapid impulses must accumulate velocity → glide further than
    /// one impulse. This is the momentum stacking property that
    /// distinguishes kinetic from position-chase animators.
    #[test]
    fn kinetic_impulses_accumulate() {
        let mut single = ScrollAnimator::new_kinetic(0.0);
        single.add_impulse(50.0);
        for _ in 0..200 {
            single.tick(50.0, NO_VP, Duration::from_millis(2));
        }

        let mut double = ScrollAnimator::new_kinetic(0.0);
        double.add_impulse(50.0);
        // Small gap (rapid keypress simulation).
        for _ in 0..4 {
            double.tick(100.0, NO_VP, Duration::from_millis(2));
        }
        double.add_impulse(50.0);
        for _ in 0..200 {
            double.tick(100.0, NO_VP, Duration::from_millis(2));
        }

        assert!(
            double.current() > single.current() * 1.5,
            "double impulse should travel further: single={}, double={}",
            single.current(),
            double.current()
        );
    }

    /// `set_landing(target)` must override velocity so the asymptotic
    /// glide settles at target — discarding any prior momentum (matches
    /// iOS scroll-to-top behavior on absolute jumps).
    #[test]
    fn kinetic_set_landing_overrides_velocity() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        a.add_impulse(1000.0); // big downward kick
        let v_before = kinetic_velocity(&a);
        // Now jump to a landing 100 px upward of current.
        a.set_landing(-100.0);
        let v_after = kinetic_velocity(&a);
        assert!(v_before > 0.0 && v_after < 0.0, "v sign should flip");
        // Glide to settle.
        for _ in 0..200 {
            a.tick(-100.0, NO_VP, Duration::from_millis(2));
        }
        assert!(
            (a.current() - (-100.0)).abs() < 0.5,
            "should land at -100, got {}",
            a.current()
        );
    }

    /// A long-running tick sequence must reach `is_animating == false`.
    #[test]
    fn kinetic_settles() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        a.set_landing(100.0);
        for _ in 0..1000 {
            a.tick(100.0, NO_VP, Duration::from_millis(2));
        }
        assert!(!a.is_animating(100.0), "kinetic did not settle");
        assert!(
            (a.current() - 100.0).abs() < 1.0,
            "settled far from target: {}",
            a.current()
        );
    }

    /// Snap requires BOTH residual<0.5 AND |v|<KINETIC_SNAP_VELOCITY:
    /// fast-glide passing through target must not prematurely report settled.
    #[test]
    fn kinetic_does_not_snap_while_fast() {
        // Direct construction to set high velocity at target position.
        let a = ScrollAnimator::Kinetic {
            current: 100.0,
            velocity: 500.0, // well above KINETIC_SNAP_VELOCITY
            tau_ms: DEFAULT_KINETIC_TAU_MS,
        };
        assert!(
            a.is_animating(100.0),
            "kinetic at target with high velocity must still animate"
        );
    }

    #[test]
    fn kinetic_zero_dt_is_noop() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        a.add_impulse(100.0);
        let c = a.tick(100.0, NO_VP, Duration::ZERO);
        assert_eq!(c, 0.0);
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

    /// Symmetric no-op assertions for `set_landing`.
    #[test]
    fn set_landing_is_noop_for_exp_decay() {
        let mut a = ScrollAnimator::new_exp_decay(10.0);
        a.set_landing(999.0);
        assert_eq!(a.current(), 10.0);
    }

    #[test]
    fn set_landing_is_noop_for_exp_decay_adaptive() {
        let mut a = ScrollAnimator::new_exp_decay_adaptive(10.0);
        a.set_landing(999.0);
        assert_eq!(a.current(), 10.0);
    }

    #[test]
    fn from_config_dispatches_kinetic() {
        let a = ScrollAnimator::from_config(7.0, crate::config::ScrollAnimation::Kinetic);
        assert!(matches!(
            a,
            ScrollAnimator::Kinetic { current, velocity, .. }
                if current == 7.0 && velocity == 0.0
        ));
    }
}
