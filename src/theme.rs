/// A set of theme data files: `(virtual filename, bytes)` pairs.
pub type DataFiles = &'static [(&'static str, &'static [u8])];

/// A built-in theme entry with all required metadata.
///
/// Adding a new theme requires filling every field; a missing field
/// causes a compile error — no silent fallback to wrong colours.
pub struct ThemeEntry {
    pub name: &'static str,
    pub source: &'static str,
    pub data_files: DataFiles,
    /// Sidebar background colour (CSS hex, e.g. `"#1e1e2e"`).
    pub sidebar_bg: &'static str,
    /// Sidebar text colour (CSS hex, e.g. `"#6c7086"`).
    pub sidebar_fg: &'static str,
}

/// Built-in themes, embedded at compile time.
pub const THEMES: &[ThemeEntry] = &[
    ThemeEntry {
        name: "catppuccin",
        source: include_str!("../themes/catppuccin.typ"),
        data_files: &[(
            "catppuccin-mocha.tmTheme",
            include_bytes!("../themes/catppuccin-mocha.tmTheme"),
        )],
        sidebar_bg: "#1e1e2e", // Mocha Base
        sidebar_fg: "#6c7086", // Mocha Overlay0
    },
    ThemeEntry {
        name: "catppuccin-latte",
        source: include_str!("../themes/catppuccin-latte.typ"),
        data_files: &[(
            "catppuccin-latte.tmTheme",
            include_bytes!("../themes/catppuccin-latte.tmTheme"),
        )],
        sidebar_bg: "#e6e9ef", // Latte Mantle
        sidebar_fg: "#8c8fa1", // Latte Overlay0
    },
];

/// Default theme name.
pub const DEFAULT_THEME: &str = "auto";

/// Resolve "auto" theme name based on terminal brightness.
pub fn resolve_theme_name(name: &str, is_light: bool) -> &str {
    if name == "auto" {
        if is_light {
            "catppuccin-latte"
        } else {
            "catppuccin"
        }
    } else {
        name
    }
}

fn find(name: &str) -> Option<&'static ThemeEntry> {
    THEMES.iter().find(|t| t.name == name)
}

/// Look up a built-in theme by name.
pub fn get(name: &str) -> Option<&'static str> {
    find(name).map(|t| t.source)
}

/// Return the data files for a theme (virtual filename → bytes).
pub fn data_files(name: &str) -> DataFiles {
    find(name).map(|t| t.data_files).unwrap_or(&[])
}

/// Return `(background, foreground)` sidebar colours for a theme.
pub fn sidebar_colors(name: &str) -> (&'static str, &'static str) {
    find(name)
        .map(|t| (t.sidebar_bg, t.sidebar_fg))
        .unwrap_or(("#1e1e2e", "#6c7086"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_auto() {
        assert_eq!(DEFAULT_THEME, "auto");
    }

    #[test]
    fn unknown_theme_returns_none() {
        assert!(get("nonexistent").is_none());
    }

    #[test]
    fn sidebar_colors_known_theme() {
        let (bg, fg) = sidebar_colors("catppuccin");
        assert_eq!(bg, "#1e1e2e");
        assert_eq!(fg, "#6c7086");
    }

    #[test]
    fn sidebar_colors_latte() {
        let (bg, fg) = sidebar_colors("catppuccin-latte");
        assert_eq!(bg, "#e6e9ef");
        assert_eq!(fg, "#8c8fa1");
    }

    #[test]
    fn sidebar_colors_unknown_falls_back() {
        let (bg, fg) = sidebar_colors("nonexistent");
        assert_eq!(bg, "#1e1e2e");
        assert_eq!(fg, "#6c7086");
    }

    #[test]
    fn resolve_auto_dark() {
        assert_eq!(resolve_theme_name("auto", false), "catppuccin");
    }

    #[test]
    fn resolve_auto_light() {
        assert_eq!(resolve_theme_name("auto", true), "catppuccin-latte");
    }

    #[test]
    fn resolve_explicit_theme() {
        assert_eq!(resolve_theme_name("catppuccin", true), "catppuccin");
        assert_eq!(
            resolve_theme_name("catppuccin-latte", false),
            "catppuccin-latte"
        );
    }
}
