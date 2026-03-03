/// Built-in themes, embedded at compile time.
pub const THEMES: &[(&str, &str)] = &[("catppuccin", include_str!("../themes/catppuccin.typ"))];

/// Default theme name.
pub const DEFAULT_THEME: &str = "catppuccin";

/// A set of theme data files: `(virtual filename, bytes)` pairs.
pub type DataFiles = &'static [(&'static str, &'static [u8])];

/// Look up a built-in theme by name.
pub fn get(name: &str) -> Option<&'static str> {
    THEMES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// Additional data files (e.g. tmTheme) required by each theme.
const THEME_DATA_FILES: &[(&str, DataFiles)] = &[(
    "catppuccin",
    &[(
        "catppuccin-mocha.tmTheme",
        include_bytes!("../themes/catppuccin-mocha.tmTheme"),
    )],
)];

/// Return the data files for a theme (virtual filename → bytes).
pub fn data_files(name: &str) -> DataFiles {
    THEME_DATA_FILES
        .iter()
        .find(|&(n, _)| *n == name)
        .map(|(_, files)| *files)
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_exists() {
        assert!(get(DEFAULT_THEME).is_some());
    }

    #[test]
    fn unknown_theme_returns_none() {
        assert!(get("nonexistent").is_none());
    }
}
