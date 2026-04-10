use std::time::Duration;

use log::debug;

// ---------------------------------------------------------------------------
// Config — resolved (all fields concrete)
// ---------------------------------------------------------------------------

pub struct Config {
    pub theme: String,
    pub width: f64,
    pub ppi: f32,
    pub viewer: ViewerConfig,
}

pub struct ViewerConfig {
    pub scroll_step: u32,
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
            viewer: ViewerConfig::default(),
        }
    }
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            scroll_step: 3,
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
    }
}

// ---------------------------------------------------------------------------
// CliOverrides — values from CLI args
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct CliOverrides {
    pub theme: Option<String>,
    pub width: Option<f64>,
    pub ppi: Option<f32>,
    pub tile_height: Option<f64>,
    pub allow_remote_images: bool,
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
            allow_remote_images: false,
        };
        config.apply_cli(&cli);
        assert_eq!(config.theme, "dark");
        assert_eq!(config.ppi, 288.0);
        assert_eq!(config.width, 660.0); // unchanged
    }
}
