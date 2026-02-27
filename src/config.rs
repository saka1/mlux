use std::path::PathBuf;
use std::time::Duration;

use log::{debug, info};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// ConfigFile — deserialized from TOML (all fields optional)
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    pub theme: Option<String>,
    pub width: Option<f64>,
    pub ppi: Option<f32>,
    #[serde(default)]
    pub viewer: ViewerConfigFile,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ViewerConfigFile {
    pub scroll_step: Option<u32>,
    pub frame_budget_ms: Option<u64>,
    pub tile_height: Option<f64>,
    pub sidebar_cols: Option<u16>,
    pub evict_distance: Option<usize>,
    pub watch_interval_ms: Option<u64>,
}

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

impl ConfigFile {
    /// Merge CLI values (overwrites non-None fields).
    pub fn merge_cli(
        &mut self,
        theme: Option<String>,
        width: Option<f64>,
        ppi: Option<f32>,
        tile_height: Option<f64>,
    ) {
        if let Some(ref v) = theme {
            debug!("config: CLI override theme={v}");
            self.theme = theme;
        }
        if let Some(v) = width {
            debug!("config: CLI override width={v}");
            self.width = width;
        }
        if let Some(v) = ppi {
            debug!("config: CLI override ppi={v}");
            self.ppi = ppi;
        }
        if let Some(v) = tile_height {
            debug!("config: CLI override tile_height={v}");
            self.viewer.tile_height = tile_height;
        }
    }

    /// Resolve to a Config by applying defaults to missing fields.
    pub fn resolve(self) -> Config {
        let config = Config {
            theme: self.theme.unwrap_or_else(|| "catppuccin".into()),
            width: self.width.unwrap_or(660.0),
            ppi: self.ppi.unwrap_or(144.0),
            viewer: ViewerConfig {
                scroll_step: self.viewer.scroll_step.unwrap_or(3),
                frame_budget: Duration::from_millis(
                    self.viewer.frame_budget_ms.unwrap_or(32),
                ),
                tile_height: self.viewer.tile_height.unwrap_or(500.0),
                sidebar_cols: self.viewer.sidebar_cols.unwrap_or(6),
                evict_distance: self.viewer.evict_distance.unwrap_or(4),
                watch_interval: Duration::from_millis(
                    self.viewer.watch_interval_ms.unwrap_or(200),
                ),
            },
        };
        info!(
            "config: resolved theme={}, width={}, ppi={}, scroll_step={}, \
             tile_height={}, sidebar_cols={}, evict_distance={}, \
             frame_budget={}ms, watch_interval={}ms",
            config.theme,
            config.width,
            config.ppi,
            config.viewer.scroll_step,
            config.viewer.tile_height,
            config.viewer.sidebar_cols,
            config.viewer.evict_distance,
            config.viewer.frame_budget.as_millis(),
            config.viewer.watch_interval.as_millis(),
        );
        config
    }
}

/// Resolve the XDG config path for mlux.
fn config_path() -> Option<PathBuf> {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(config_dir.join("mlux").join("config.toml"))
}

/// Load config file. Returns `ConfigFile::default()` if no file exists.
/// Returns an error if the file exists but cannot be parsed.
pub fn load_config() -> anyhow::Result<ConfigFile> {
    let path = match config_path() {
        Some(p) => p,
        None => {
            info!("config: no HOME or XDG_CONFIG_HOME set, using defaults");
            return Ok(ConfigFile::default());
        }
    };
    debug!("config: looking for {}", path.display());
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            info!("config: loaded from {}", path.display());
            let cfg: ConfigFile = toml::from_str(&text)
                .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
            Ok(cfg)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("config: {} not found, using defaults", path.display());
            Ok(ConfigFile::default())
        }
        Err(e) => Err(anyhow::anyhow!("failed to read {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml() {
        let cfg: ConfigFile = toml::from_str("").unwrap();
        let resolved = cfg.resolve();
        assert_eq!(resolved.theme, "catppuccin");
        assert_eq!(resolved.width, 660.0);
        assert_eq!(resolved.ppi, 144.0);
        assert_eq!(resolved.viewer.scroll_step, 3);
        assert_eq!(resolved.viewer.sidebar_cols, 6);
        assert_eq!(resolved.viewer.evict_distance, 4);
    }

    #[test]
    fn partial_toml() {
        let text = r#"
            ppi = 288.0
            [viewer]
            scroll_step = 10
        "#;
        let cfg: ConfigFile = toml::from_str(text).unwrap();
        let resolved = cfg.resolve();
        assert_eq!(resolved.ppi, 288.0);
        assert_eq!(resolved.viewer.scroll_step, 10);
        // Defaults for unspecified fields
        assert_eq!(resolved.theme, "catppuccin");
        assert_eq!(resolved.width, 660.0);
        assert_eq!(resolved.viewer.sidebar_cols, 6);
    }

    #[test]
    fn invalid_toml() {
        let text = "this is not valid toml [[[";
        let result = toml::from_str::<ConfigFile>(text);
        assert!(result.is_err());
    }

    #[test]
    fn cli_overrides() {
        let mut cfg: ConfigFile = toml::from_str("ppi = 100.0").unwrap();
        cfg.merge_cli(Some("dark".into()), None, Some(288.0), None);
        let resolved = cfg.resolve();
        assert_eq!(resolved.theme, "dark");
        assert_eq!(resolved.ppi, 288.0); // CLI wins
        assert_eq!(resolved.width, 660.0); // default (neither config nor CLI)
    }
}
