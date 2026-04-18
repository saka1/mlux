use std::time::Duration;

use log::debug;

// ---------------------------------------------------------------------------
// Config — resolved (all fields concrete)
// ---------------------------------------------------------------------------

pub struct Config {
    pub theme: String,
    pub width: f64,
    pub ppi: f32,
    pub scale: f64,
    pub viewer: ViewerConfig,
}

/// User-facing scroll-behavior choice.  Config carries only the
/// selection; the viewer maps each variant to a concrete strategy
/// implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollMode {
    /// Constant step per keypress (classic behavior).
    #[default]
    Fixed,
    /// Input-density-driven multiplier (experimental).
    Adaptive,
}

/// Downstream interpolation algorithm selection — chooses the
/// `ScrollAnimator` variant used to advance `current → target` each
/// frame. Independent of [`ScrollMode`] (which governs the upstream
/// target-delta accumulation layer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollAnimation {
    /// Exponential decay (closed-form), the v1 baseline.
    #[default]
    ExpDecay,
    /// Exponential decay with distance-adaptive half-life.
    /// `hl(d) = base × (1 + ln(1 + d/viewport))` — near distances
    /// behave like `ExpDecay`, large jumps stretch sub-linearly so
    /// gg/G stays trackable (design doc §3.4).
    ExpDecayAdaptive,
}

pub struct ViewerConfig {
    pub scroll_step: u32,
    pub scroll_mode: ScrollMode,
    pub scroll_animation: ScrollAnimation,
    pub frame_budget: Duration,
    pub tile_height: f64,
    pub sidebar_cols: u16,
    pub evict_distance: usize,
    pub watch_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.into(),
            width: 660.0,
            ppi: 144.0,
            scale: 1.0,
            viewer: ViewerConfig::default(),
        }
    }
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            scroll_step: 3,
            scroll_mode: ScrollMode::default(),
            scroll_animation: ScrollAnimation::default(),
            frame_budget: Duration::from_millis(32),
            tile_height: 500.0,
            sidebar_cols: 6,
            evict_distance: 4,
            watch_interval: Duration::from_millis(200),
        }
    }
}

impl Config {
    /// Apply CLI overrides to this config.
    pub fn apply_cli(&mut self, cli: &CliOverrides) {
        if let Some(ref v) = cli.theme {
            debug!("config: CLI override theme={v}");
            self.theme = v.clone();
        }
        if let Some(v) = cli.width {
            debug!("config: CLI override width={v}");
            self.width = v;
        }
        if let Some(v) = cli.ppi {
            debug!("config: CLI override ppi={v}");
            self.ppi = v;
        }
        if let Some(v) = cli.tile_height {
            debug!("config: CLI override tile_height={v}");
            self.viewer.tile_height = v;
        }
        if let Some(v) = cli.scale {
            debug!("config: CLI override scale={v}");
            self.scale = v;
        }
        if let Some(mode) = cli.scroll_mode {
            debug!("config: CLI override scroll_mode={mode:?}");
            self.viewer.scroll_mode = mode;
            // Note: `scroll_step` is NOT coerced here.  It applies only
            // to the `Fixed` strategy; `Adaptive` uses its own internal
            // base (see `src/viewer/scroll_policy.rs`) and ignores this
            // setting entirely.  Earlier revisions coerced `scroll_step
            // = 2` for adaptive, which mixed user preference with
            // algorithm tuning — keeping them disjoint is the fix.
        }
        if let Some(anim) = cli.scroll_animation {
            debug!("config: CLI override scroll_animation={anim:?}");
            self.viewer.scroll_animation = anim;
        }
    }
}

// ---------------------------------------------------------------------------
// CliOverrides — values from CLI args
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct CliOverrides {
    pub theme: Option<String>,
    pub width: Option<f64>,
    pub ppi: Option<f32>,
    pub tile_height: Option<f64>,
    pub scale: Option<f64>,
    pub allow_remote_images: bool,
    pub scroll_mode: Option<ScrollMode>,
    pub scroll_animation: Option<ScrollAnimation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let config = Config::default();
        assert_eq!(config.theme, "auto");
        assert_eq!(config.width, 660.0);
        assert_eq!(config.ppi, 144.0);
        assert_eq!(config.viewer.scroll_step, 3);
        assert_eq!(config.viewer.scroll_mode, ScrollMode::Fixed);
        assert_eq!(config.viewer.sidebar_cols, 6);
        assert_eq!(config.viewer.evict_distance, 4);
    }

    #[test]
    fn cli_overrides() {
        let mut config = Config::default();
        let cli = CliOverrides {
            theme: Some("dark".into()),
            width: None,
            ppi: Some(288.0),
            tile_height: None,
            scale: None,
            allow_remote_images: false,
            scroll_mode: None,
            scroll_animation: None,
        };
        config.apply_cli(&cli);
        assert_eq!(config.theme, "dark");
        assert_eq!(config.ppi, 288.0);
        assert_eq!(config.width, 660.0); // unchanged
        assert_eq!(config.scale, 1.0); // unchanged
    }

    #[test]
    fn scale_default_and_override() {
        let mut config = Config::default();
        assert_eq!(config.scale, 1.0);
        let cli = CliOverrides {
            scale: Some(1.5),
            ..Default::default()
        };
        config.apply_cli(&cli);
        assert_eq!(config.scale, 1.5);
    }

    #[test]
    fn scroll_animation_override() {
        let mut config = Config::default();
        assert_eq!(config.viewer.scroll_animation, ScrollAnimation::ExpDecay);
        let cli = CliOverrides {
            scroll_animation: Some(ScrollAnimation::ExpDecay),
            ..Default::default()
        };
        config.apply_cli(&cli);
        assert_eq!(config.viewer.scroll_animation, ScrollAnimation::ExpDecay);
    }

    #[test]
    fn scroll_mode_override_does_not_touch_scroll_step() {
        // `scroll_step` is the user's canonical preference; selecting
        // adaptive mode must not silently rewrite it.  The adaptive
        // algorithm scales off whatever the user configured.
        for mode in [ScrollMode::Fixed, ScrollMode::Adaptive] {
            let mut config = Config::default();
            assert_eq!(config.viewer.scroll_step, 3);
            let cli = CliOverrides {
                scroll_mode: Some(mode),
                ..Default::default()
            };
            config.apply_cli(&cli);
            assert_eq!(config.viewer.scroll_mode, mode);
            assert_eq!(config.viewer.scroll_step, 3); // unchanged
        }
    }
}
