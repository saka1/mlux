# Scroll Animator Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract scroll animation logic from `src/viewer/layout.rs` into a new `src/viewer/scroll_animator.rs` module, behind a closed-enum `ScrollAnimator` API, as the foundation for future pluggable interpolation algorithms (spring, bezier, ramp-up). Behavior must be preserved exactly: same half-life (40ms), same snap threshold (0.5px), same frame-rate-independent exponential decay.

**Architecture:** `layout.rs` regains its original responsibility ("static geometry + position/bounds snapshot") per `docs/2026-03-07-design-viewer-state.md`. A new `scroll_animator.rs` owns the time-evolution domain (current→target interpolation, snap thresholds, per-algorithm state). `ScrollState` keeps `y_offset`/`target_y`/bounds but delegates motion to an owned `ScrollAnimator`. The `ScrollAnimator::ExpDecay` variant is the only algorithm for now; adding future variants means extending the enum and compiler-driven `match` updates — same pattern as `ScrollStrategy` in `src/viewer/scroll.rs`.

**Tech Stack:** Rust 2024, `std::time::Duration`. No new dependencies.

**Out of scope:** Adding new animation algorithms (spring, bezier). Config/CLI integration for algorithm selection. Distance-adaptive half-life. All deferred to follow-up plans — this plan only establishes the module boundary and enum shape.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `src/viewer/scroll_animator.rs` | **Create** | `ScrollAnimator` enum, algorithm implementations, animation unit tests |
| `src/viewer/layout.rs` | Modify | Remove `interpolate_step`, `SCROLL_HALF_LIFE_MS`, 5 animation tests. `ScrollState` loses `current_y` field, gains `animator: ScrollAnimator` field, gains `ScrollState::new()` constructor. `tick` and `is_animating` become thin delegators. |
| `src/viewer/mod.rs` | Modify | Add `mod scroll_animator;` declaration. Update `ScrollState` construction at 3-argument site to use `ScrollState::new()`. |
| `src/viewer/viewport.rs` | Modify | Update `Default` impl to use `ScrollState::new()`. `Effect::ScrollTo` also calls `animator.set_target()` (no-op for ExpDecay but the hook must exist for future variants). |
| `src/viewer/test_harness.rs` | Modify | Update construction to use `ScrollState::new()`. |
| `src/viewer/mode_normal.rs` | Modify | Update test-only `make_state` helper and `scroll_accumulates_onto_target_not_render_position` test's mid-animation simulation (both reference `current_y` directly). |

Touch count: 1 new file, 5 modified files. Public API of `ScrollState` changes (`current_y` field removed, `new()` constructor added), so all construction sites must migrate in the same commit to stay green.

---

## Task 1: Extract scroll animation into its own module

**Files:**
- Create: `src/viewer/scroll_animator.rs`
- Modify: `src/viewer/layout.rs` (remove animation internals, keep position/bounds)
- Modify: `src/viewer/mod.rs` (add module declaration, update construction site)
- Modify: `src/viewer/viewport.rs` (Default impl, `Effect::ScrollTo` hook)
- Modify: `src/viewer/test_harness.rs` (construction site)
- Modify: `src/viewer/mode_normal.rs` (test helper + test)

This is a single behavior-preserving refactor committed as one atomic change. Broken into small steps so progress is checkpointable; the final commit comes after all steps pass.

- [ ] **Step 1: Baseline — verify current tests pass**

Run:
```bash
cargo test --quiet
```

Expected: all tests pass (383 unit + 61 integration + 5 usecase = 449). If any failures here, stop and investigate — the refactor must be compared against a known-green baseline.

- [ ] **Step 2: Create `src/viewer/scroll_animator.rs`**

Create the file with the full `ScrollAnimator` enum and its implementation. This is the new home for animation logic.

```rust
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
            Self::ExpDecay { current, half_life_ms } => {
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
        let c = a.tick(100.0, Duration::from_secs_f64(DEFAULT_HALF_LIFE_MS / 1000.0));
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
```

- [ ] **Step 3: Register the new module in `src/viewer/mod.rs`**

Add the `mod scroll_animator;` declaration alphabetically between `scroll_policy` and `session`. The existing declarations are in `src/viewer/mod.rs:20-42`:

```rust
mod scroll;
mod scroll_policy;
mod session;
```

Change to:
```rust
mod scroll;
mod scroll_animator;
mod scroll_policy;
mod session;
```

- [ ] **Step 4: Verify new module compiles and tests pass**

Run:
```bash
cargo test scroll_animator --quiet
```

Expected: 8 new tests pass (half-life, frame-rate independence, zero-dt, snap, convergence, negative direction, set_target no-op, is_animating threshold).

If there are `dead_code` warnings about `ScrollAnimator` methods being unused at this point, ignore — they will be consumed by `ScrollState` in later steps.

- [ ] **Step 5: Migrate `ScrollState` in `src/viewer/layout.rs` — struct definition**

Open `src/viewer/layout.rs`. The current `ScrollState` at lines 27-39 carries `current_y: f64` and imports/uses `interpolate_step`. Replace it with a version that owns a `ScrollAnimator`.

Add near the top of the file, after the existing `use std::time::Duration;`:

```rust
use super::scroll_animator::ScrollAnimator;
```

Replace the struct definition (currently lines 22-39) with:

```rust
/// Scroll position and viewport/document pixel dimensions.
///
/// Groups the data needed for scroll-bound calculations: current rendered
/// position (`y_offset`), desired position (`target_y`), document height
/// (`img_h`), viewport dimensions (`vp_w`, `vp_h`), and the animation
/// strategy driving `y_offset` toward `target_y` each frame.
///
/// The animator owns the sub-pixel position; `y_offset` is the rounded
/// integer readers depend on.
pub(super) struct ScrollState {
    /// Rendered integer position (pixels). Updated by `tick()` from the
    /// animator's current sub-pixel position. Read widely by downstream
    /// code (status bar, visible_tiles, prefetch, etc.).
    pub y_offset: u32,
    /// Desired final position set by `Effect::ScrollTo`.
    pub target_y: u32,
    pub img_h: u32, // ドキュメント高さ（ピクセル）
    pub vp_w: u32,  // ビューポート幅（ピクセル）
    pub vp_h: u32,  // ビューポート高さ（ピクセル）
    /// Animation strategy. Advances `y_offset` toward `target_y` each tick.
    pub animator: ScrollAnimator,
}
```

- [ ] **Step 6: Migrate `ScrollState` in `src/viewer/layout.rs` — methods**

Replace the `impl ScrollState` block (currently lines 41-62) with:

```rust
impl ScrollState {
    /// Construct a `ScrollState` at rest at `initial_y` (no pending animation).
    pub fn new(initial_y: u32, img_h: u32, vp_w: u32, vp_h: u32) -> Self {
        Self {
            y_offset: initial_y,
            target_y: initial_y,
            img_h,
            vp_w,
            vp_h,
            animator: ScrollAnimator::new_exp_decay(initial_y as f64),
        }
    }

    /// Whether an in-flight animation is still running.
    pub fn is_animating(&self) -> bool {
        self.animator.is_animating(self.target_y as f64)
    }

    /// Advance the animation one frame. Returns true if `y_offset` (the
    /// rendered integer position) changed — callers use this to decide
    /// whether to redraw.
    pub fn tick(&mut self, dt: Duration) -> bool {
        let prev = self.y_offset;
        let current = self.animator.tick(self.target_y as f64, dt);
        self.y_offset = current.round() as u32;
        self.y_offset != prev
    }
}
```

- [ ] **Step 7: Remove animation internals from `src/viewer/layout.rs`**

Delete the following from `src/viewer/layout.rs`:

1. The `SCROLL_HALF_LIFE_MS` const (currently line 115 with its doc comment 113-114).
2. The `interpolate_step` function (currently lines 117-128 with doc comment).
3. The five `interpolate_step_*` tests inside `mod tests`:
   - `interpolate_step_half_life_behavior` (lines 156-165)
   - `interpolate_step_frame_rate_independent` (lines 167-179)
   - `interpolate_step_zero_dt_is_noop` (lines 181-185)
   - `interpolate_step_converges_toward_target` (lines 187-193)
   - `interpolate_step_handles_negative_direction` (lines 195-203)

These tests' intent is covered by the new tests in `scroll_animator.rs` (Step 2).

Keep the other tests in `layout.rs`: `compute_layout_*`, `visual_line_offset_*`, `vp_dims_*`, `layout_pt_conversions`, `align_tile_height_rounds_up_to_cell_boundary`. They are pure geometry, unrelated to animation.

After this edit, `src/viewer/layout.rs` no longer mentions animation — it speaks only `Layout`, `ScrollState` (as a data carrier), and pure geometry helpers.

- [ ] **Step 8: Update `ScrollState` construction in `src/viewer/mod.rs`**

Currently `src/viewer/mod.rs:287-297` constructs `ScrollState` with explicit fields:

```rust
scroll: {
    let y = session.scroll_carry.min(meta.max_scroll(vp_h));
    ScrollState {
        y_offset: y,
        current_y: y as f64,
        target_y: y,
        img_h,
        vp_w,
        vp_h,
    }
},
```

Replace with:

```rust
scroll: ScrollState::new(
    session.scroll_carry.min(meta.max_scroll(vp_h)),
    img_h,
    vp_w,
    vp_h,
),
```

- [ ] **Step 9: Update `Viewport::default` construction in `src/viewer/viewport.rs`**

Currently `src/viewer/viewport.rs:31-50` has:

```rust
impl Default for Viewport {
    fn default() -> Self {
        Self {
            mode: ViewerMode::Normal,
            scroll: ScrollState {
                y_offset: 0,
                current_y: 0.0,
                target_y: 0,
                img_h: 0,
                vp_w: 0,
                vp_h: 0,
            },
            display: DisplayState::new(0),
            flash: None,
            dirty: false,
            last_search: None,
            highlights_visible: true,
        }
    }
}
```

Replace the `scroll` initializer:

```rust
scroll: ScrollState::new(0, 0, 0, 0),
```

- [ ] **Step 10: Route `Effect::ScrollTo` through the animator's `set_target` hook**

Currently `src/viewer/viewport.rs:73-79`:

```rust
Effect::ScrollTo(y) => {
    // Only update the target — the inner loop steps current_y toward it
    // each frame (sub-cell resolution). Split-case compression artifacts
    // are handled per-frame in the redraw path, not by snapping here.
    self.scroll.target_y = y;
    self.dirty = true;
}
```

Replace with:

```rust
Effect::ScrollTo(y) => {
    // Only update the target — the animator steps toward it each frame
    // (sub-cell resolution). Split-case compression artifacts are handled
    // per-frame in the redraw path, not by snapping here. `set_target`
    // is a no-op for ExpDecay but future animators (Bezier, ramp-up)
    // reset internal timers here.
    self.scroll.target_y = y;
    self.scroll.animator.set_target(y as f64);
    self.dirty = true;
}
```

- [ ] **Step 11: Update `ScrollState` construction in `src/viewer/test_harness.rs`**

Currently `src/viewer/test_harness.rs:74-89`:

```rust
let viewport = Viewport {
    mode: ViewerMode::Normal,
    scroll: ScrollState {
        y_offset: 0,
        current_y: 0.0,
        target_y: 0,
        img_h: meta.total_height_px,
        vp_w,
        vp_h,
    },
    display: DisplayState::new(4),
    flash: None,
    dirty: false,
    last_search: None,
    highlights_visible: true,
};
```

Replace the `scroll` initializer:

```rust
scroll: ScrollState::new(0, meta.total_height_px, vp_w, vp_h),
```

- [ ] **Step 12: Update test helpers in `src/viewer/mode_normal.rs`**

This module has a test helper (`make_state`, currently lines 354-363) and one test (`scroll_accumulates_onto_target_not_render_position`, currently lines 365-383) that poke at `current_y` directly. Both need updates.

First, add an import at the top of the `#[cfg(test)] mod tests` block near the other `use` statements. Look for existing imports like `use super::*;` and add:

```rust
use super::super::scroll_animator::ScrollAnimator;
```

Replace the `make_state` helper (currently lines 354-363):

```rust
fn make_state(y_offset: u32) -> ScrollState {
    ScrollState {
        y_offset,
        target_y: y_offset,
        img_h: 2000,
        vp_w: 800,
        vp_h: 600,
        animator: ScrollAnimator::new_exp_decay(y_offset as f64),
    }
}
```

Then update the body of `scroll_accumulates_onto_target_not_render_position` (currently lines 365-383). The old code manually sets `state.current_y = 40.0` to simulate mid-animation. That field no longer exists; instead, reseat the animator at 40.0:

```rust
#[test]
fn scroll_accumulates_onto_target_not_render_position() {
    // Simulate mid-animation: render position (y_offset / animator.current)
    // is lagged behind target_y. A new ScrollDown must stack onto target_y
    // so rapid keypresses accumulate correctly.
    let mut state = make_state(0);
    state.target_y = 100;
    state.animator = ScrollAnimator::new_exp_decay(40.0);
    state.y_offset = 40;
    let vls = vec![make_vl(0)];
    let ci = empty_ci();
    let doc = DocumentQuery::new("", &vls, &ci, 0);
    let mut ls = None;
    let mut ctx = make_ctx(&state, &doc, &mut ls);
    ctx.scroll_step = 50;
    let effects = handle(Action::ScrollDown(1), &mut ctx);
    // Expected: 100 (target) + 50 (step) = 150. NOT 40 (render) + 50 = 90.
    assert!(matches!(effects[0], Effect::ScrollTo(y) if y == 150));
}
```

- [ ] **Step 13: Run format + clippy + tests**

Run the three quality gates (project convention, see `CLAUDE.md`):

```bash
cargo fmt
cargo clippy --all-targets
cargo test --quiet
```

Expected:
- `cargo fmt`: no changes (or only whitespace formatting).
- `cargo clippy --all-targets`: zero warnings.
- `cargo test`: all tests pass. The 5 removed `interpolate_step_*` tests are replaced by 8 new `exp_decay_*` / `set_target_*` / `is_animating_*` tests in `scroll_animator.rs`; total count goes up by 3.

If any test fails or any clippy warning appears, fix root cause — do not `#[allow]` or mask. Likely failure modes:
- Forgot to update one construction site → compile error pointing at the file.
- `current_y` leaked into a new caller since the plan was written → grep `current_y` under `src/` and migrate.
- `mode_normal.rs` import path for `ScrollAnimator` is wrong → inspect the existing `use` structure and adjust.

Verify the old code is fully gone:

```bash
grep -rn "interpolate_step\|SCROLL_HALF_LIFE_MS\|current_y" src/
```

Expected output: zero matches (all three identifiers should be gone from `src/`).

- [ ] **Step 14: Sanity-check viewer behavior manually (golden path)**

Build and run the viewer against a real document. This is a behavior-preserving refactor — scrolling must feel identical to before.

```bash
cargo run -- README.md
```

In the viewer:
- Press `j` a few times. Scrolling should animate smoothly, same feel as before (half-life 40ms).
- Press `G` to jump to end. Should animate to the bottom.
- Press `gg` to jump to top. Should animate to the top.
- Press `q` to quit.

If motion feels wrong (snaps instantly, no animation, or oscillates), inspect the animator wiring — `Effect::ScrollTo` must call `set_target`, `ScrollState::tick` must run each outer loop iteration (`src/viewer/mod.rs:361`).

- [ ] **Step 15: Commit**

```bash
git add src/viewer/scroll_animator.rs src/viewer/layout.rs src/viewer/mod.rs src/viewer/viewport.rs src/viewer/test_harness.rs src/viewer/mode_normal.rs docs/2026-04-19-plan-scroll-animator-extraction.md
git commit -m "$(cat <<'EOF'
refactor(viewer): extract scroll animation into pluggable module

Move time-evolution logic (interpolate_step, SCROLL_HALF_LIFE_MS, snap
threshold) out of layout.rs into a new scroll_animator.rs. layout.rs
regains its original single responsibility — static geometry plus the
position/bounds snapshot per docs/2026-03-07-design-viewer-state.md.

ScrollAnimator is a closed enum (same convention as ScrollStrategy in
scroll.rs), currently with one variant: ExpDecay. The set_target hook
is a no-op for ExpDecay but the API shape is ready for Bezier / spring
/ ramp-up variants that need to reset internal timers on target change.

Behavior-preserving: same half-life (40ms), same 0.5px snap, same
frame-rate-independent math. Five interpolate_step_* unit tests moved
into scroll_animator.rs and expanded to cover the new API surface.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Then verify the commit landed cleanly:

```bash
git log -1 --stat
```

Expected: one commit touching 6 source files and adding 1 plan doc, with a clear diff showing animation code moved between files.

---

## Self-Review

**Spec coverage:**
- Extract animation out of layout.rs → Steps 2, 5-7 ✓
- Preserve behavior exactly → Step 14 golden-path check + Steps 2, 7 preserving math and constants ✓
- Closed enum pluggable API with `set_target`/`tick`/`is_animating` → Step 2 (API surface) ✓
- Rewire all callers without breaking tests → Steps 8-12 ✓
- Quality gates (fmt/clippy --all-targets/test) → Step 13 ✓

**Placeholder scan:** No TBDs, no "implement X", no "similar to above". Every code block contains the exact content to write or remove.

**Type consistency:**
- `ScrollAnimator` is `pub(super)` in `scroll_animator.rs` — imported as `super::scroll_animator::ScrollAnimator` in `layout.rs` and as `super::super::scroll_animator::ScrollAnimator` in `mode_normal.rs`'s nested test module. Both paths valid.
- `ScrollState::new(initial_y: u32, img_h: u32, vp_w: u32, vp_h: u32)` signature identical across Steps 6, 8, 9, 11 — same argument order everywhere.
- `ScrollAnimator::new_exp_decay(initial: f64)` identical across Steps 2, 6, 12.
- `set_target(&mut self, target: f64)` / `tick(&mut self, target: f64, dt: Duration) -> f64` / `is_animating(&self, target: f64) -> bool` — signatures consistent between definition (Step 2) and call sites (Steps 6, 10).

**Key invariants preserved:**
- `y_offset = animator.current().round() as u32` — enforced in `ScrollState::tick` (Step 6).
- Snap at residual < 0.5px — moved from `ScrollState::tick` into `ScrollAnimator::tick` (Step 2), behavior identical.
- `is_animating` threshold 0.5px — moved from hardcoded `ScrollState::is_animating` body into `ScrollAnimator::is_animating` (Step 2), same value.
- `SCROLL_HALF_LIFE_MS = 40.0` → `DEFAULT_HALF_LIFE_MS = 40.0` (renamed, same value, same semantics).

---

## Notes for Follow-up Plans

This plan deliberately stops at the boundary. Once merged, these are the natural next PRs:

1. **Distance-adaptive half-life** (`docs/2026-04-18-experiment-subcell-scroll.md` §4.2 (1)) — add a second variant `ExpDecayAdaptive { current, base_half_life_ms, jump_distance }` and branch per `set_target` to scale half-life by `log(1 + d/viewport)`. Config default remains `ExpDecay`.
2. **Alternative algorithms** — `Spring { current, velocity, k, c }` for critical-damped; `Bezier { t0, start, target, duration, curve }` for fixed-time. Each adds one variant + its `match` arms + tests.
3. **Config/CLI integration** — `AnimationMode` enum in `src/config.rs`, `--anim` flag, `Config::apply_cli` wiring, so users can A/B compare at runtime without recompiling.

All three are independent and can ship in any order once this extraction lands.
