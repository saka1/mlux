use std::path::PathBuf;

use crate::config::{CliOverrides, Config};
use crate::pipeline::{BuildParams, FontCache};
use crate::theme::{self, DataFiles};

/// Resolved theme: name, source text, and data files.
pub struct ResolvedTheme {
    pub name: String,
    pub text: &'static str,
    pub data_files: DataFiles,
}

/// Immutable application context shared by render and viewer modes.
///
/// Created via `AppContextBuilder`. Owns the expensive, reusable state
/// (FontCache, Config, resolved theme) that both modes need.
pub struct AppContext {
    pub font_cache: &'static FontCache,
    pub config: Config,
    pub cli_overrides: CliOverrides,
    pub detected_light: bool,
    pub theme: ResolvedTheme,
}

impl AppContext {
    /// Construct `BuildParams` from this context plus mode-specific dimensions.
    ///
    /// Render provides fixed width from CLI; viewer computes from terminal size.
    /// Fields `ppi`, `allow_remote_images`, theme, and fonts come from `self`.
    pub fn build_params(
        &self,
        markdown: String,
        base_dir: Option<PathBuf>,
        width_pt: f64,
        sidebar_width_pt: f64,
        tile_height_pt: f64,
    ) -> BuildParams {
        BuildParams {
            theme_name: self.theme.name.clone(),
            theme_text: self.theme.text.to_string(),
            data_files: self.theme.data_files,
            markdown,
            base_dir,
            width_pt,
            sidebar_width_pt,
            tile_height_pt,
            ppi: self.config.ppi,
            fonts: self.font_cache,
            allow_remote_images: self.cli_overrides.allow_remote_images,
        }
    }
}

/// Mutable builder that collects initialization state step by step.
///
/// Semantics: application initialization = collecting state into an
/// `AppContext` via this builder.
pub struct AppContextBuilder {
    config: Config,
    cli_overrides: CliOverrides,
    font_cache: Option<&'static FontCache>,
    detected_light: Option<bool>,
}

impl AppContextBuilder {
    /// Start building from a fresh config.
    pub fn new(config: Config, cli_overrides: CliOverrides) -> Self {
        Self {
            config,
            cli_overrides,
            font_cache: None,
            detected_light: None,
        }
    }

    /// Rebuild from an existing `AppContext` on config reload.
    ///
    /// Consumes the old context to reclaim `FontCache` and `detected_light`.
    /// Only theme resolution will be re-executed on `.build()`.
    pub fn from_existing(
        new_config: Config,
        new_cli_overrides: CliOverrides,
        old: &AppContext,
    ) -> Self {
        Self {
            config: new_config,
            cli_overrides: new_cli_overrides,
            font_cache: Some(old.font_cache),
            detected_light: Some(old.detected_light),
        }
    }

    /// Load system + embedded fonts (one-time ~13ms filesystem scan).
    pub fn load_fonts(mut self) -> Self {
        self.font_cache = Some(Box::leak(Box::new(FontCache::new())));
        self
    }

    /// Set terminal theme detection result.
    ///
    /// Caller is responsible for detection (may need raw mode).
    /// Defaults to `false` (dark) if not called.
    pub fn set_detected_light(mut self, is_light: bool) -> Self {
        self.detected_light = Some(is_light);
        self
    }

    /// Consume the builder and produce an immutable `AppContext`.
    ///
    /// Resolves the theme internally based on `config.theme` + `detected_light`.
    /// Returns `Err` if the resolved theme name is unknown.
    /// Panics if `load_fonts()` was not called.
    pub fn build(self) -> anyhow::Result<AppContext> {
        let font_cache = self
            .font_cache
            .expect("load_fonts() must be called before build()");
        let detected_light = self.detected_light.unwrap_or(false);

        let resolved_name =
            theme::resolve_theme_name(&self.config.theme, detected_light).to_string();
        let text = theme::get(&resolved_name)
            .ok_or_else(|| anyhow::anyhow!("unknown theme '{resolved_name}'"))?;
        let data_files = theme::data_files(&resolved_name);

        Ok(AppContext {
            font_cache,
            config: self.config,
            cli_overrides: self.cli_overrides,
            detected_light,
            theme: ResolvedTheme {
                name: resolved_name,
                text,
                data_files,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliOverrides, Config};

    fn test_config() -> Config {
        let cfg = crate::config::ConfigFile::default();
        cfg.resolve()
    }

    fn test_overrides() -> CliOverrides {
        CliOverrides {
            theme: None,
            width: None,
            ppi: None,
            tile_height: None,
            allow_remote_images: false,
        }
    }

    #[test]
    fn build_with_default_theme() {
        let app = AppContextBuilder::new(test_config(), test_overrides())
            .load_fonts()
            .build()
            .unwrap();
        // Default config theme is "auto", detected_light defaults to false → "catppuccin"
        assert_eq!(app.theme.name, "catppuccin");
        assert!(!app.theme.text.is_empty());
        assert!(!app.detected_light);
    }

    #[test]
    fn build_with_light_detection() {
        let app = AppContextBuilder::new(test_config(), test_overrides())
            .load_fonts()
            .set_detected_light(true)
            .build()
            .unwrap();
        // auto + light → catppuccin-latte
        assert_eq!(app.theme.name, "catppuccin-latte");
    }

    #[test]
    fn build_with_explicit_theme() {
        let mut config = test_config();
        config.theme = "catppuccin-latte".to_string();
        let app = AppContextBuilder::new(config, test_overrides())
            .load_fonts()
            .build()
            .unwrap();
        assert_eq!(app.theme.name, "catppuccin-latte");
    }

    #[test]
    fn build_with_unknown_theme_fails() {
        let mut config = test_config();
        config.theme = "nonexistent-theme-xyz".to_string();
        let result = AppContextBuilder::new(config, test_overrides())
            .load_fonts()
            .build();
        assert!(result.is_err());
    }

    #[test]
    #[should_panic(expected = "load_fonts() must be called")]
    fn build_without_fonts_panics() {
        let _ = AppContextBuilder::new(test_config(), test_overrides()).build();
    }

    #[test]
    fn from_existing_preserves_font_cache_and_detected_light() {
        let app = AppContextBuilder::new(test_config(), test_overrides())
            .load_fonts()
            .set_detected_light(true)
            .build()
            .unwrap();

        let mut new_config = test_config();
        new_config.theme = "catppuccin-latte".to_string();
        let new_overrides = test_overrides();

        let app2 = AppContextBuilder::from_existing(new_config, new_overrides, &app)
            .build()
            .unwrap();

        assert!(app2.detected_light); // preserved
        assert_eq!(app2.theme.name, "catppuccin-latte"); // re-resolved
    }
}
