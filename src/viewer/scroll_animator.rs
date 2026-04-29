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

use std::time::{Duration, Instant};

use super::input_history::InputHistory;

/// Default half-life for the ExpDecay algorithm (ms).
///
/// The residual distance to target halves every `DEFAULT_HALF_LIFE_MS`,
/// regardless of frame rate. Tuned empirically — see
/// `docs/2026-04-29-experiments-scroll-animation.md` Phase 1.
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

/// Pure parameters for the Kinetic animator.  Persists no state — every
/// query is a closed-form function of `(anchor, history, now)`.
///
/// Physical model: `dv/dt = -v/τ` with impulses at times tᵢ adding
/// `δᵢ/τ` to velocity.  Integrating gives the closed forms below;
/// they are exact for any `dt` (frame-rate independent by construction).
///
/// ```text
/// x(t) = anchor + Σᵢ δᵢ · (1 - e^(-(t - tᵢ)/τ))
/// v(t) = Σᵢ (δᵢ / τ) · e^(-(t - tᵢ)/τ)
/// ```
///
/// Old impulses (>5τ) contribute essentially their full δ to position
/// and ~0 to velocity; eviction from the input history convolves their
/// δ into a permanent anchor (see `viewport.rs`).
#[derive(Clone, Copy, Debug)]
pub(super) struct KineticParams {
    pub tau_ms: f64,
}

impl KineticParams {
    pub fn new() -> Self {
        Self {
            tau_ms: DEFAULT_KINETIC_TAU_MS,
        }
    }

    /// Closed-form position at `now` given `anchor` and the impulse
    /// history recorded so far.
    pub fn position_at(&self, anchor: f64, history: &InputHistory, now: Instant) -> f64 {
        let tau_s = self.tau_ms / 1000.0;
        anchor
            + history
                .iter()
                .map(|r| {
                    let elapsed = now.saturating_duration_since(r.timestamp).as_secs_f64();
                    r.delta_px as f64 * (1.0 - (-elapsed / tau_s).exp())
                })
                .sum::<f64>()
    }

    /// Closed-form velocity (px/s) at `now`.
    pub fn velocity_at(&self, history: &InputHistory, now: Instant) -> f64 {
        let tau_s = self.tau_ms / 1000.0;
        history
            .iter()
            .map(|r| {
                let elapsed = now.saturating_duration_since(r.timestamp).as_secs_f64();
                (r.delta_px as f64 / tau_s) * (-elapsed / tau_s).exp()
            })
            .sum()
    }
}

/// Pluggable scroll interpolation.
///
/// All variants share the unified [`Self::tick`] API
/// `(anchor, history, viewport, now, dt) → f64`.  Position-chase
/// variants (ExpDecay / ExpDecayAdaptive) compute their target as
/// `anchor + Σ history.delta_px` and update internal `current` per
/// frame.  Kinetic is stateless: its position is a closed-form
/// function of `(anchor, history, now)` (see [`KineticParams`]).
pub(super) enum ScrollAnimator {
    /// Exponential decay toward target with ease-in ramp on new scroll.
    ///
    /// Closed-form of `dx/dt = -λ(t)(x - target)` where `λ(t)` ramps from
    /// `ln(2) / (RAMP_INITIAL_SCALE × half_life_ms)` up to
    /// `ln(2) / half_life_ms` over `RAMP_DURATION_MS` via smoothstep.
    /// After the ramp the frame-rate-independence property holds.
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
    /// pure friction decay.  Stateless: position and velocity at any
    /// `now` are evaluated from `(anchor, history)` via [`KineticParams`].
    ///
    /// Impulses arrive as history entries (`Effect::ScrollImpulse`).
    /// `Effect::ScrollAnchor` flushes history and pins anchor to the
    /// current position, then re-pushes a single landing impulse —
    /// equivalent to the legacy iOS "scroll-to-top" `set_landing`
    /// semantics, but without persistent velocity state.
    Kinetic(KineticParams),
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

    /// Construct a Kinetic animator.  Initial position is carried by
    /// `ScrollState::anchor`, not the animator (the variant is stateless).
    pub(super) fn new_kinetic(_initial: f64) -> Self {
        Self::Kinetic(KineticParams::new())
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

    /// Current sub-pixel position at `now` given `anchor` and `history`.
    /// For Kinetic this is a closed-form evaluation; for ExpDecay
    /// variants it returns the animator's internal `current`.
    pub(super) fn current_position(
        &self,
        anchor: f64,
        history: &InputHistory,
        now: Instant,
    ) -> f64 {
        match self {
            Self::ExpDecay { current, .. } | Self::ExpDecayAdaptive { current, .. } => *current,
            Self::Kinetic(params) => params.position_at(anchor, history, now),
        }
    }

    /// Whether the animator has motion left toward the derived target
    /// (`anchor + Σ history.delta_px`).
    ///
    /// Returns false once residual is below [`SNAP_THRESHOLD_PX`].
    /// `Kinetic` additionally requires |velocity| below
    /// [`KINETIC_SNAP_VELOCITY`] so a fast glide passing through target
    /// doesn't prematurely report settled.
    pub(super) fn is_animating(&self, anchor: f64, history: &InputHistory, now: Instant) -> bool {
        let target_sum: i64 = history.iter().map(|r| r.delta_px as i64).sum();
        let target = anchor + target_sum as f64;
        match self {
            Self::ExpDecay { current, .. } | Self::ExpDecayAdaptive { current, .. } => {
                (*current - target).abs() >= SNAP_THRESHOLD_PX
            }
            Self::Kinetic(params) => {
                let x = params.position_at(anchor, history, now);
                let v = params.velocity_at(history, now);
                !((target - x).abs() < SNAP_THRESHOLD_PX && v.abs() < KINETIC_SNAP_VELOCITY)
            }
        }
    }

    /// Anchor contribution for an evicted history record.
    ///
    /// When a record is evicted (by time-window or cap), its displacement
    /// is folded into the permanent anchor.  For Kinetic the record's
    /// *current* contribution to position is `δ·(1 - e^(-elapsed/τ))`
    /// rather than the full `δ` — the residual `δ·e^(-elapsed/τ)` is
    /// still in-flight.  For ExpDecay variants the history isn't part of
    /// the position formula, so the full `δ` is correct.
    pub(super) fn eviction_contribution(
        &self,
        record: &super::input_history::InputRecord,
        now: Instant,
    ) -> f64 {
        match self {
            Self::Kinetic(params) => {
                let tau_s = params.tau_ms / 1000.0;
                let elapsed = now
                    .saturating_duration_since(record.timestamp)
                    .as_secs_f64();
                record.delta_px as f64 * (1.0 - (-elapsed / tau_s).exp())
            }
            _ => record.delta_px as f64,
        }
    }

    /// Reset the ExpDecay ease-in ramp.  No-op for other variants.
    ///
    /// Called from the apply layer when entering a new scroll from a
    /// settled state so the eye gets the pursuit-onset ramp; while
    /// already scrolling the ramp is left intact.
    pub(super) fn restart_ease_in_if_settled(&mut self, settled: bool) {
        if let Self::ExpDecay {
            ramp_elapsed_ms, ..
        } = self
            && settled
        {
            *ramp_elapsed_ms = 0.0;
        }
    }

    /// Advance one frame toward the derived target (`anchor + Σ
    /// history.delta_px`) over elapsed `dt`.  Returns the new sub-pixel
    /// position.
    ///
    /// `viewport_px` is consumed by distance-adaptive variants;
    /// fixed-half-life variants and Kinetic ignore it.  Kinetic also
    /// ignores `dt` — its position is a pure closed-form function of
    /// `(anchor, history, now)`.
    pub(super) fn tick(
        &mut self,
        anchor: f64,
        history: &InputHistory,
        viewport_px: f64,
        now: Instant,
        dt: Duration,
    ) -> f64 {
        let target_sum: i64 = history.iter().map(|r| r.delta_px as i64).sum();
        let target = anchor + target_sum as f64;
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
                let v = viewport_px.max(1.0);
                let hl = *base_half_life_ms * (1.0 + (1.0 + d / v).ln());
                let dt_ms = dt.as_secs_f64() * 1000.0;
                let alpha = 1.0 - 0.5_f64.powf(dt_ms / hl);
                apply_step(current, target, alpha)
            }
            Self::Kinetic(params) => {
                // Pure: position is a closed-form function of (anchor,
                // history, now).  No internal state, dt unused.
                let _ = dt;
                params.position_at(anchor, history, now)
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
    use super::super::input_history::ScrollDirection;
    use super::*;

    /// Viewport height irrelevant for ExpDecay; use an obvious sentinel.
    const NO_VP: f64 = 0.0;

    fn empty_history() -> InputHistory {
        InputHistory::new(Duration::from_secs(5), 128)
    }

    // ---- KineticParams (history-driven, pure) -----------------------------

    #[test]
    fn kinetic_params_position_is_anchor_with_no_history() {
        let params = KineticParams::new();
        let h = empty_history();
        let x = params.position_at(123.5, &h, Instant::now());
        assert_eq!(x, 123.5);
    }

    #[test]
    fn kinetic_params_position_asymptotes_to_anchor_plus_sum() {
        // After many τ, residual ≈ 0 → x ≈ anchor + Σδᵢ.
        let params = KineticParams::new();
        let mut h = empty_history();
        let t0 = Instant::now();
        let _ = h.record(ScrollDirection::Down, 72);
        let later = t0 + Duration::from_secs(5); // ≫ 5τ
        let x = params.position_at(0.0, &h, later);
        assert!((x - 72.0).abs() < 0.01, "x = {x}");
    }

    #[test]
    fn kinetic_params_one_tau_progress() {
        // After exactly τ, position should equal anchor + δ * (1 - 1/e).
        let params = KineticParams::new();
        let mut h = empty_history();
        let t0 = Instant::now();
        let _ = h.record(ScrollDirection::Down, 100);
        let later = t0 + Duration::from_secs_f64(DEFAULT_KINETIC_TAU_MS / 1000.0);
        let x = params.position_at(0.0, &h, later);
        let expected = 100.0 * (1.0 - (-1.0_f64).exp());
        assert!(
            (x - expected).abs() < 0.5,
            "x = {x} vs expected = {expected}"
        );
    }

    #[test]
    fn kinetic_params_velocity_decays_with_tau() {
        let params = KineticParams::new();
        let mut h = empty_history();
        let t0 = Instant::now();
        let _ = h.record(ScrollDirection::Down, 1000);
        let v0 = params.velocity_at(&h, t0); // ≈ 1000 / 0.05 = 20000 px/s
        let v1 = params.velocity_at(
            &h,
            t0 + Duration::from_secs_f64(DEFAULT_KINETIC_TAU_MS / 1000.0),
        );
        let ratio = v1 / v0;
        let expected = (-1.0_f64).exp(); // 1/e
        assert!(
            (ratio - expected).abs() < 0.01,
            "v1/v0 = {ratio} vs 1/e = {expected}"
        );
    }

    #[test]
    fn kinetic_params_impulses_accumulate_in_position() {
        // Two impulses must asymptotically land at anchor + δ₁ + δ₂.
        let params = KineticParams::new();
        let mut h = empty_history();
        let _ = h.record(ScrollDirection::Down, 50);
        let _ = h.record(ScrollDirection::Down, 50);
        let later = Instant::now() + Duration::from_secs(5);
        let x = params.position_at(0.0, &h, later);
        assert!((x - 100.0).abs() < 0.5, "x = {x}");
    }

    #[test]
    fn kinetic_params_signed_deltas_can_brake() {
        // Down impulse followed by Up impulse should asymptote to net.
        let params = KineticParams::new();
        let mut h = empty_history();
        let _ = h.record(ScrollDirection::Down, 100);
        let _ = h.record(ScrollDirection::Up, -40);
        let later = Instant::now() + Duration::from_secs(5);
        let x = params.position_at(0.0, &h, later);
        assert!((x - 60.0).abs() < 0.5, "x = {x}");
    }

    // ---- ExpDecay (target-chase, history is empty) ------------------------

    /// Wrap a position-chase tick: anchor=target, no history, current Instant.
    fn tick_chase(a: &mut ScrollAnimator, target: f64, vp: f64, dt: Duration) -> f64 {
        let h = empty_history();
        a.tick(target, &h, vp, Instant::now(), dt)
    }

    fn position_chase_current(a: &ScrollAnimator) -> f64 {
        a.current_position(0.0, &empty_history(), Instant::now())
    }

    fn is_animating_chase(a: &ScrollAnimator, target: f64) -> bool {
        a.is_animating(target, &empty_history(), Instant::now())
    }

    #[test]
    fn exp_decay_half_life_behavior() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        let c = tick_chase(
            &mut a,
            100.0,
            NO_VP,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        assert!((c - 50.0).abs() < 0.01, "expected ~50, got {c}");
    }

    #[test]
    fn exp_decay_frame_rate_independent() {
        let full = Duration::from_millis(40);
        let half = Duration::from_millis(20);

        let mut one = ScrollAnimator::new_exp_decay(0.0);
        let one_shot = tick_chase(&mut one, 100.0, NO_VP, full);

        let mut two = ScrollAnimator::new_exp_decay(0.0);
        tick_chase(&mut two, 100.0, NO_VP, half);
        let two_shot = tick_chase(&mut two, 100.0, NO_VP, half);

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
        let c = tick_chase(&mut a, 100.0, NO_VP, Duration::ZERO);
        assert_eq!(c, 10.0);
        assert_eq!(position_chase_current(&a), 10.0);
    }

    #[test]
    fn exp_decay_snaps_when_residual_is_subpixel() {
        let mut a = ScrollAnimator::ExpDecay {
            current: 99.9,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
            ramp_elapsed_ms: RAMP_DURATION_MS,
        };
        let c = tick_chase(&mut a, 100.0, NO_VP, Duration::from_millis(40));
        assert_eq!(c, 100.0);
        assert!(!is_animating_chase(&a, 100.0));
    }

    #[test]
    fn exp_decay_converges_toward_target() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        let dt = Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS * 10.0 / 1000.0);
        let c = tick_chase(&mut a, 100.0, NO_VP, dt);
        assert_eq!(c, 100.0);
    }

    #[test]
    fn exp_decay_handles_negative_direction() {
        let mut a = ScrollAnimator::new_exp_decay(100.0);
        let c = tick_chase(
            &mut a,
            0.0,
            NO_VP,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        assert!((c - 50.0).abs() < 0.01, "expected ~50, got {c}");
    }

    #[test]
    fn restart_ease_in_does_not_change_position() {
        let mut a = ScrollAnimator::new_exp_decay(10.0);
        a.restart_ease_in_if_settled(true);
        assert_eq!(position_chase_current(&a), 10.0);
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
        let a = ScrollAnimator::ExpDecay {
            current: 99.4,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
            ramp_elapsed_ms: RAMP_DURATION_MS,
        };
        assert!(is_animating_chase(&a, 100.0));
        let b = ScrollAnimator::ExpDecay {
            current: 99.6,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
            ramp_elapsed_ms: RAMP_DURATION_MS,
        };
        assert!(!is_animating_chase(&b, 100.0));
    }

    // ---- ExpDecay ease-in ramp --------------------------------------------

    #[test]
    fn exp_decay_ramp_starts_slow() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        a.restart_ease_in_if_settled(true); // trigger ease-in
        let c = tick_chase(
            &mut a,
            100.0,
            NO_VP,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        // Steady-state (no ramp) would give ~50. Ramp must give less.
        assert!(c < 50.0, "expected ramp to slow start, got {c}");
    }

    #[test]
    fn exp_decay_ramp_completes_after_100ms() {
        let ramp_ms = RAMP_DURATION_MS as u64;
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        a.restart_ease_in_if_settled(true);
        for _ in 0..10 {
            tick_chase(&mut a, 100.0, NO_VP, Duration::from_millis(ramp_ms / 10));
        }
        let residual_before = 100.0 - position_chase_current(&a);
        let c = tick_chase(
            &mut a,
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

    #[test]
    fn exp_decay_no_ramp_reset_when_already_animating() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        a.restart_ease_in_if_settled(true);
        tick_chase(&mut a, 100.0, NO_VP, Duration::from_millis(50));
        let elapsed_mid = match &a {
            ScrollAnimator::ExpDecay {
                ramp_elapsed_ms, ..
            } => *ramp_elapsed_ms,
            _ => panic!(),
        };
        a.restart_ease_in_if_settled(false);
        let elapsed_after = match &a {
            ScrollAnimator::ExpDecay {
                ramp_elapsed_ms, ..
            } => *ramp_elapsed_ms,
            _ => panic!(),
        };
        assert_eq!(elapsed_mid, elapsed_after, "ramp should not reset");
    }

    #[test]
    fn exp_decay_ramp_resets_on_new_scroll_from_settled() {
        let mut a = ScrollAnimator::new_exp_decay(0.0);
        tick_chase(&mut a, 100.0, NO_VP, Duration::from_millis(200));
        a.restart_ease_in_if_settled(true);
        let elapsed = match &a {
            ScrollAnimator::ExpDecay {
                ramp_elapsed_ms, ..
            } => *ramp_elapsed_ms,
            _ => panic!(),
        };
        assert_eq!(elapsed, 0.0, "ramp_elapsed should reset to 0");
    }

    // ---- ExpDecayAdaptive -------------------------------------------------

    #[test]
    fn adaptive_half_life_at_one_viewport() {
        let viewport = 1000.0;
        let mut a = ScrollAnimator::new_exp_decay_adaptive(0.0);
        let expected_hl_ms = DEFAULT_HALF_LIFE_MS * (1.0 + 2.0_f64.ln());
        let c = tick_chase(
            &mut a,
            1000.0,
            viewport,
            Duration::from_secs_f64(expected_hl_ms / 1000.0),
        );
        assert!(
            (c - 500.0).abs() < 0.5,
            "expected ~500 after one adaptive half-life, got {c}"
        );
    }

    #[test]
    fn adaptive_near_distance_matches_base_half_life() {
        let viewport = 10_000.0;
        let mut a = ScrollAnimator::ExpDecayAdaptive {
            current: 0.0,
            base_half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = tick_chase(
            &mut a,
            50.0,
            viewport,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        assert!(
            (c - 25.0).abs() < 0.25,
            "expected ~25 for near-distance, got {c}"
        );
    }

    #[test]
    fn adaptive_large_jump_decays_slower_than_base() {
        let viewport = 100.0;
        let mut a = ScrollAnimator::new_exp_decay_adaptive(0.0);
        let c = tick_chase(
            &mut a,
            1000.0,
            viewport,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        assert!(
            c < 500.0,
            "adaptive should under-progress vs fixed, got {c}"
        );
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
        let c = tick_chase(&mut a, 100.0, 500.0, Duration::ZERO);
        assert_eq!(c, 10.0);
    }

    #[test]
    fn adaptive_snaps_when_residual_is_subpixel() {
        let mut a = ScrollAnimator::ExpDecayAdaptive {
            current: 99.9,
            base_half_life_ms: DEFAULT_HALF_LIFE_MS,
        };
        let c = tick_chase(&mut a, 100.0, 500.0, Duration::from_millis(40));
        assert_eq!(c, 100.0);
        assert!(!is_animating_chase(&a, 100.0));
    }

    #[test]
    fn adaptive_zero_viewport_is_safe() {
        let mut a = ScrollAnimator::new_exp_decay_adaptive(0.0);
        let c = tick_chase(
            &mut a,
            100.0,
            0.0,
            Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0),
        );
        assert!(c > 0.0 && c < 100.0, "got {c}");
    }

    // ---- Kinetic (history-driven, equivalent to legacy semantics) --------

    /// A single impulse from rest must asymptote to anchor + δ.  The
    /// legacy `add_impulse(72.0)` behavior is recovered by pushing
    /// (Down, 72) to history.
    #[test]
    fn kinetic_glides_to_anchor_plus_delta() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        let mut h = empty_history();
        let _ = h.record(ScrollDirection::Down, 72);
        let later = Instant::now() + Duration::from_secs(5);
        let x = a.tick(0.0, &h, NO_VP, later, Duration::from_millis(10));
        assert!((x - 72.0).abs() < 0.5, "kinetic should land at 72, got {x}");
    }

    /// Frame-rate independence is automatic: position is a closed-form
    /// function of (anchor, history, now), so two half-dt sub-ticks
    /// give the same result as one full-dt tick.
    #[test]
    fn kinetic_frame_rate_independent() {
        let mut h = empty_history();
        let _ = h.record(ScrollDirection::Down, 100);
        let t0 = Instant::now();
        let dt_full = Duration::from_millis(20);
        let dt_half = Duration::from_millis(10);

        let mut one = ScrollAnimator::new_kinetic(0.0);
        let one_shot = one.tick(0.0, &h, NO_VP, t0 + dt_full, dt_full);

        let mut two = ScrollAnimator::new_kinetic(0.0);
        two.tick(0.0, &h, NO_VP, t0 + dt_half, dt_half);
        let two_shot = two.tick(0.0, &h, NO_VP, t0 + dt_full, dt_half);

        assert!(
            (one_shot - two_shot).abs() < 1e-9,
            "one_shot={one_shot} two_shot={two_shot}"
        );
    }

    /// Two rapid impulses must accumulate position contribution.
    #[test]
    fn kinetic_impulses_accumulate() {
        let mut single = empty_history();
        let _ = single.record(ScrollDirection::Down, 50);

        let mut double = empty_history();
        let _ = double.record(ScrollDirection::Down, 50);
        let _ = double.record(ScrollDirection::Down, 50);

        let later = Instant::now() + Duration::from_secs(5);
        let mut a = ScrollAnimator::new_kinetic(0.0);
        let single_x = a.tick(0.0, &single, NO_VP, later, Duration::from_millis(2));
        let double_x = a.tick(0.0, &double, NO_VP, later, Duration::from_millis(2));
        assert!(
            double_x > single_x * 1.5,
            "double impulse should travel further: single={single_x}, double={double_x}"
        );
    }

    /// `Effect::ScrollAnchor`-style landing: the apply layer drains the
    /// in-flight history, pins anchor to the current position, and
    /// re-pushes a single (target - current) impulse.  This is the
    /// closed-form equivalent of legacy `set_landing`.
    #[test]
    fn kinetic_landing_via_drain_and_repush() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        let mut h = empty_history();
        // First flick downward.
        let _ = h.record(ScrollDirection::Down, 1000);

        // ...some time passes, user fires gg → simulate apply()'s drain
        // + anchor pin + single landing impulse.
        let now = Instant::now();
        let current = a.current_position(0.0, &h, now);
        h.drain();
        let anchor = current;
        let target = -100.0;
        let delta = (target as i32) - (current.round() as i32);
        let _ = h.record(ScrollDirection::Up, delta);

        // Glide to settle.
        let later = now + Duration::from_secs(5);
        let x = a.tick(anchor, &h, NO_VP, later, Duration::from_millis(2));
        assert!(
            (x - target).abs() < 1.0,
            "should land near {target}, got {x}"
        );
    }

    /// Long evolution past 5τ must reach `is_animating == false`.
    #[test]
    fn kinetic_settles() {
        let a = ScrollAnimator::new_kinetic(0.0);
        let mut h = empty_history();
        let _ = h.record(ScrollDirection::Down, 100);
        let later = Instant::now() + Duration::from_secs(5);
        assert!(!a.is_animating(0.0, &h, later), "kinetic did not settle");
    }

    /// Snap requires BOTH residual<0.5 AND |v|<KINETIC_SNAP_VELOCITY:
    /// fast-glide passing through target must not prematurely report settled.
    #[test]
    fn kinetic_does_not_snap_while_fast() {
        // At time t with single big impulse just fired, x ≈ 0 but v is
        // huge → still animating even though x might be at the target.
        let a = ScrollAnimator::new_kinetic(0.0);
        let mut h = empty_history();
        let t0 = Instant::now();
        let _ = h.record(ScrollDirection::Down, 1000);
        // Sample at exactly t0 — position contribution = 0 (asymptote
        // factor is 1 - exp(0) = 0), velocity = 1000/τ = 20000 px/s.
        // Target = anchor + Σδ = 1000.  Residual = 1000.  Definitely animating.
        assert!(a.is_animating(0.0, &h, t0));
    }

    #[test]
    fn kinetic_zero_dt_is_noop() {
        let mut a = ScrollAnimator::new_kinetic(0.0);
        let h = empty_history();
        let now = Instant::now();
        let c = a.tick(0.0, &h, NO_VP, now, Duration::ZERO);
        assert_eq!(c, 0.0);
    }

    #[test]
    fn from_config_dispatches_kinetic() {
        let a = ScrollAnimator::from_config(7.0, crate::config::ScrollAnimation::Kinetic);
        assert!(matches!(a, ScrollAnimator::Kinetic(_)));
    }

    /// Pin the "bit-equivalent to the legacy stateful Kinetic" claim.
    ///
    /// The legacy implementation carried persistent `(current, velocity)`
    /// and integrated `dv/dt = -v/τ` step-wise: between events
    /// `v_new = v·e^(-dt/τ)`, `x += (v - v_new)·τ`; an impulse δ added
    /// `δ/τ` to v.  The new closed form must agree exactly with that
    /// recurrence for any timestamp set, since both are exact solutions
    /// of the same linear ODE.  We replay the recurrence over the
    /// timestamps actually recorded in `InputHistory` (so the comparison
    /// is independent of how the impulses got spaced) and check
    /// agreement at several sample points covering pre-settle and
    /// post-settle regimes.
    #[test]
    fn kinetic_matches_legacy_stateful_recurrence() {
        use std::thread;

        let params = KineticParams::new();
        let tau_s = params.tau_ms / 1000.0;

        let mut h = empty_history();
        let _ = h.record(ScrollDirection::Down, 72);
        thread::sleep(Duration::from_millis(15));
        let _ = h.record(ScrollDirection::Down, 50);
        thread::sleep(Duration::from_millis(20));
        let _ = h.record(ScrollDirection::Up, -30);

        let records: Vec<_> = h.iter().copied().collect();
        let t_first = records[0].timestamp;

        // Walk the legacy recurrence to `sample`, returning (x, v).
        let legacy_walk = |sample: Instant| -> (f64, f64) {
            let mut x = 0.0_f64;
            let mut v = 0.0_f64;
            let mut t = t_first;
            for r in &records {
                let dt = r.timestamp.saturating_duration_since(t).as_secs_f64();
                if dt > 0.0 {
                    let decay = (-dt / tau_s).exp();
                    let v_new = v * decay;
                    x += (v - v_new) * tau_s;
                    v = v_new;
                    t = r.timestamp;
                }
                // Impulse: velocity bumps by δ/τ, position unchanged.
                v += r.delta_px as f64 / tau_s;
            }
            let dt = sample.saturating_duration_since(t).as_secs_f64();
            if dt > 0.0 {
                let decay = (-dt / tau_s).exp();
                let v_new = v * decay;
                x += (v - v_new) * tau_s;
                v = v_new;
            }
            (x, v)
        };

        // Sample at several offsets past the last recorded impulse:
        //   25ms (pre-settle), 100ms (~2τ, mid), 500ms (~10τ, settled).
        let last_t = records.last().unwrap().timestamp;
        for offset_ms in [25_u64, 100, 500] {
            let sample = last_t + Duration::from_millis(offset_ms);
            let (legacy_x, legacy_v) = legacy_walk(sample);
            let closed_x = params.position_at(0.0, &h, sample);
            let closed_v = params.velocity_at(&h, sample);
            assert!(
                (closed_x - legacy_x).abs() < 1e-9,
                "position diverged at +{offset_ms}ms: closed={closed_x} legacy={legacy_x}",
            );
            assert!(
                (closed_v - legacy_v).abs() < 1e-9,
                "velocity diverged at +{offset_ms}ms: closed={closed_v} legacy={legacy_v}",
            );
        }
    }
}
