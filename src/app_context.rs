use std::path::PathBuf;

use crate::compile::FontCache;
use crate::config::{CliOverrides, Config};
use crate::pipeline::BuildParams;
use crate::theme;

/// Immutable application context shared by render and viewer modes.
///
/// Created via `AppContextBuilder`. Owns the expensive, reusable state
/// (FontCache, Config) that both modes need.
/// Does NOT depend on document content — theme resolution happens at build time.
pub struct AppContext {
    pub font_cache: &'static FontCache,
    pub config: Config,
    pub cli_overrides: CliOverrides,
    pub detected_light: bool,
}

impl AppContext {
    /// Construct `BuildParams` from this context plus mode-specific dimensions.
    ///
    /// Render provides fixed width from CLI; viewer computes from terminal size.
    /// Fields `ppi`, `allow_remote_images`, and fonts come from `self`.
    /// Theme resolution is deferred to the build pipeline.
    #[allow(clippy::too_many_arguments)]
    pub fn build_params(
        &self,
        markdown: String,
        base_dir: Option<PathBuf>,
        file_path: Option<PathBuf>,
        width_pt: f64,
        sidebar_width_pt: f64,
        tile_height_pt: f64,
        fast_png: bool,
    ) -> BuildParams {
        BuildParams {
            theme_spec: self.config.theme.clone(),
            detected_light: self.detected_light,
            markdown,
            base_dir,
            file_path,
            width_pt,
            sidebar_width_pt,
            tile_height_pt,
            ppi: self.config.ppi,
            scale: self.config.scale,
            fonts: self.font_cache,
            allow_remote_images: self.cli_overrides.allow_remote_images,
            fast_png,
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

    /// Rebuild from an existing `AppContext`, reusing font cache and theme detection.
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
    /// Validates that the theme specifier is known (alias or built-in name).
    /// Returns `Err` if the theme spec is invalid.
    /// Panics if `load_fonts()` was not called.
    pub fn build(self) -> anyhow::Result<AppContext> {
        let font_cache = self
            .font_cache
            .expect("load_fonts() must be called before build()");
        let detected_light = self.detected_light.unwrap_or(false);

        if !theme::is_valid_theme_spec(&self.config.theme) {
            anyhow::bail!("unknown theme '{}'", self.config.theme);
        }

        Ok(AppContext {
            font_cache,
            config: self.config,
            cli_overrides: self.cli_overrides,
            detected_light,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliOverrides, Config};

    fn test_config() -> Config {
        Config::default()
    }

    fn test_overrides() -> CliOverrides {
        CliOverrides::default()
    }

    #[test]
    fn build_with_default_theme() {
        let app = AppContextBuilder::new(test_config(), test_overrides())
            .load_fonts()
            .build()
            .unwrap();
        // Default config theme is "auto"
        assert_eq!(app.config.theme, "auto");
        assert!(!app.detected_light);
    }

    #[test]
    fn build_with_light_detection() {
        let app = AppContextBuilder::new(test_config(), test_overrides())
            .load_fonts()
            .set_detected_light(true)
            .build()
            .unwrap();
        assert!(app.detected_light);
    }

    #[test]
    fn build_with_explicit_theme() {
        let mut config = test_config();
        config.theme = "catppuccin-latte".to_string();
        let app = AppContextBuilder::new(config, test_overrides())
            .load_fonts()
            .build()
            .unwrap();
        assert_eq!(app.config.theme, "catppuccin-latte");
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
        assert_eq!(app2.config.theme, "catppuccin-latte");
    }
}
