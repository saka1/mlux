/// Built-in themes, embedded at compile time.
pub const THEMES: &[(&str, &str)] = &[("catppuccin", include_str!("../themes/catppuccin.typ"))];

/// Default theme name.
pub const DEFAULT_THEME: &str = "catppuccin";

/// Look up a built-in theme by name.
pub fn get(name: &str) -> Option<&'static str> {
    THEMES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
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
