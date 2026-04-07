/// A set of theme data files: `(virtual filename, bytes)` pairs.
pub type DataFiles = &'static [(&'static str, &'static [u8])];

/// Mermaid diagram color palette for a theme.
pub struct MermaidColors {
    pub background: &'static str,
    pub primary_color: &'static str,
    pub secondary_color: &'static str,
    pub tertiary_color: &'static str,
    pub primary_text_color: &'static str,
    pub text_color: &'static str,
    pub primary_border_color: &'static str,
    pub line_color: &'static str,
    pub edge_label_background: &'static str,
    pub cluster_background: &'static str,
    pub cluster_border: &'static str,
    pub sequence_actor_fill: &'static str,
    pub sequence_actor_border: &'static str,
    pub sequence_actor_line: &'static str,
    pub sequence_note_fill: &'static str,
    pub sequence_note_border: &'static str,
    pub sequence_activation_fill: &'static str,
    pub sequence_activation_border: &'static str,
}

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
    /// Mermaid diagram colour palette.
    pub mermaid: MermaidColors,
}

/// Mapping from base theme names to their latin variants.
const LATIN_VARIANTS: &[(&str, &str)] = &[
    ("catppuccin", "catppuccin-latin"),
    ("catppuccin-latte", "catppuccin-latte-latin"),
];

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
        sidebar_fg: "#a6adc8", // Mocha Subtext0
        mermaid: MermaidColors {
            background: "#1e1e2e",            // Base
            primary_color: "#313244",         // Surface0
            secondary_color: "#45475a",       // Surface1
            tertiary_color: "#585b70",        // Surface2
            primary_text_color: "#cdd6f4",    // Text
            text_color: "#cdd6f4",            // Text
            primary_border_color: "#585b70",  // Surface2
            line_color: "#a6adc8",            // Subtext0
            edge_label_background: "#1e1e2e", // Base
            cluster_background: "#313244",    // Surface0
            cluster_border: "#585b70",        // Surface2
            sequence_actor_fill: "#313244",
            sequence_actor_border: "#585b70",
            sequence_actor_line: "#a6adc8",
            sequence_note_fill: "#45475a",
            sequence_note_border: "#f5c2e7", // Pink
            sequence_activation_fill: "#45475a",
            sequence_activation_border: "#585b70",
        },
    },
    ThemeEntry {
        name: "catppuccin-latte",
        source: include_str!("../themes/catppuccin-latte.typ"),
        data_files: &[(
            "catppuccin-latte.tmTheme",
            include_bytes!("../themes/catppuccin-latte.tmTheme"),
        )],
        sidebar_bg: "#e6e9ef", // Latte Mantle
        sidebar_fg: "#6c6f85", // Latte Subtext0
        mermaid: MermaidColors {
            background: "#FFFFFF",
            primary_color: "#F8FAFC",
            secondary_color: "#E2E8F0",
            tertiary_color: "#FFFFFF",
            primary_text_color: "#0F172A",
            text_color: "#0F172A",
            primary_border_color: "#94A3B8",
            line_color: "#64748B",
            edge_label_background: "#FFFFFF",
            cluster_background: "#F1F5F9",
            cluster_border: "#CBD5E1",
            sequence_actor_fill: "#F8FAFC",
            sequence_actor_border: "#94A3B8",
            sequence_actor_line: "#64748B",
            sequence_note_fill: "#FFF7ED",
            sequence_note_border: "#FDBA74",
            sequence_activation_fill: "#E2E8F0",
            sequence_activation_border: "#94A3B8",
        },
    },
    ThemeEntry {
        name: "catppuccin-latin",
        source: include_str!("../themes/catppuccin-latin.typ"),
        data_files: &[(
            "catppuccin-mocha.tmTheme",
            include_bytes!("../themes/catppuccin-mocha.tmTheme"),
        )],
        sidebar_bg: "#1e1e2e",
        sidebar_fg: "#a6adc8", // Mocha Subtext0
        mermaid: MermaidColors {
            background: "#1e1e2e",
            primary_color: "#313244",
            secondary_color: "#45475a",
            tertiary_color: "#585b70",
            primary_text_color: "#cdd6f4",
            text_color: "#cdd6f4",
            primary_border_color: "#585b70",
            line_color: "#a6adc8",
            edge_label_background: "#1e1e2e",
            cluster_background: "#313244",
            cluster_border: "#585b70",
            sequence_actor_fill: "#313244",
            sequence_actor_border: "#585b70",
            sequence_actor_line: "#a6adc8",
            sequence_note_fill: "#45475a",
            sequence_note_border: "#f5c2e7",
            sequence_activation_fill: "#45475a",
            sequence_activation_border: "#585b70",
        },
    },
    ThemeEntry {
        name: "catppuccin-latte-latin",
        source: include_str!("../themes/catppuccin-latte-latin.typ"),
        data_files: &[(
            "catppuccin-latte.tmTheme",
            include_bytes!("../themes/catppuccin-latte.tmTheme"),
        )],
        sidebar_bg: "#e6e9ef",
        sidebar_fg: "#6c6f85", // Latte Subtext0
        mermaid: MermaidColors {
            background: "#FFFFFF",
            primary_color: "#F8FAFC",
            secondary_color: "#E2E8F0",
            tertiary_color: "#FFFFFF",
            primary_text_color: "#0F172A",
            text_color: "#0F172A",
            primary_border_color: "#94A3B8",
            line_color: "#64748B",
            edge_label_background: "#FFFFFF",
            cluster_background: "#F1F5F9",
            cluster_border: "#CBD5E1",
            sequence_actor_fill: "#F8FAFC",
            sequence_actor_border: "#94A3B8",
            sequence_actor_line: "#64748B",
            sequence_note_fill: "#FFF7ED",
            sequence_note_border: "#FDBA74",
            sequence_activation_fill: "#E2E8F0",
            sequence_activation_border: "#94A3B8",
        },
    },
];

/// Default theme name.
pub const DEFAULT_THEME: &str = "auto";

/// Resolve theme name aliases based on terminal brightness and CJK content.
///
/// Supported aliases: `"auto"` (detect), `"dark"`, `"light"`.
/// For alias names, applies latin variant when `!has_cjk`.
/// Explicit theme names are returned as-is.
pub fn resolve_theme_name(name: &str, is_light: bool, has_cjk: bool) -> &str {
    let base = match name {
        "auto" => {
            if is_light {
                "catppuccin-latte"
            } else {
                "catppuccin"
            }
        }
        "dark" => "catppuccin",
        "light" => "catppuccin-latte",
        _ => return name,
    };
    if !has_cjk && let Some((_, latin)) = LATIN_VARIANTS.iter().find(|(b, _)| *b == base) {
        return latin;
    }
    base
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
        .unwrap_or(("#1e1e2e", "#a6adc8"))
}

/// Return Mermaid diagram colours for a theme (falls back to catppuccin dark).
pub fn mermaid_colors(name: &str) -> &'static MermaidColors {
    find(name).map(|t| &t.mermaid).unwrap_or(&THEMES[0].mermaid)
}

/// Check if a theme specifier is valid (alias or known theme name).
///
/// Delegates to [`resolve_theme_name`] so that adding a new alias
/// automatically makes it pass validation — no separate list to sync.
pub fn is_valid_theme_spec(name: &str) -> bool {
    // Resolve with has_cjk=true to get a base theme (all Latin variants also exist).
    let resolved = resolve_theme_name(name, false, true);
    find(resolved).is_some()
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
        assert_eq!(fg, "#a6adc8");
    }

    #[test]
    fn sidebar_colors_latte() {
        let (bg, fg) = sidebar_colors("catppuccin-latte");
        assert_eq!(bg, "#e6e9ef");
        assert_eq!(fg, "#6c6f85");
    }

    #[test]
    fn sidebar_colors_unknown_falls_back() {
        let (bg, fg) = sidebar_colors("nonexistent");
        assert_eq!(bg, "#1e1e2e");
        assert_eq!(fg, "#a6adc8");
    }

    #[test]
    fn mermaid_colors_dark() {
        let mc = mermaid_colors("catppuccin");
        assert_eq!(mc.background, "#1e1e2e");
        assert_eq!(mc.primary_text_color, "#cdd6f4");
    }

    #[test]
    fn mermaid_colors_light() {
        let mc = mermaid_colors("catppuccin-latte");
        assert_eq!(mc.background, "#FFFFFF");
        assert_eq!(mc.primary_text_color, "#0F172A");
    }

    #[test]
    fn mermaid_colors_unknown_falls_back() {
        let mc = mermaid_colors("nonexistent");
        assert_eq!(mc.background, "#1e1e2e"); // falls back to catppuccin dark
    }

    #[test]
    fn resolve_auto_dark() {
        assert_eq!(resolve_theme_name("auto", false, true), "catppuccin");
    }

    #[test]
    fn resolve_auto_light() {
        assert_eq!(resolve_theme_name("auto", true, true), "catppuccin-latte");
    }

    #[test]
    fn resolve_explicit_theme() {
        assert_eq!(resolve_theme_name("catppuccin", true, true), "catppuccin");
        assert_eq!(
            resolve_theme_name("catppuccin-latte", false, true),
            "catppuccin-latte"
        );
    }

    #[test]
    fn resolve_dark_alias() {
        assert_eq!(resolve_theme_name("dark", false, true), "catppuccin");
        assert_eq!(resolve_theme_name("dark", true, true), "catppuccin");
    }

    #[test]
    fn resolve_light_alias() {
        assert_eq!(resolve_theme_name("light", false, true), "catppuccin-latte");
        assert_eq!(resolve_theme_name("light", true, true), "catppuccin-latte");
    }

    #[test]
    fn resolve_auto_dark_latin() {
        assert_eq!(resolve_theme_name("auto", false, false), "catppuccin-latin");
    }

    #[test]
    fn resolve_auto_light_latin() {
        assert_eq!(
            resolve_theme_name("auto", true, false),
            "catppuccin-latte-latin"
        );
    }

    #[test]
    fn resolve_dark_alias_latin() {
        assert_eq!(resolve_theme_name("dark", false, false), "catppuccin-latin");
    }

    #[test]
    fn resolve_light_alias_latin() {
        assert_eq!(
            resolve_theme_name("light", false, false),
            "catppuccin-latte-latin"
        );
    }

    #[test]
    fn resolve_explicit_theme_ignores_has_cjk() {
        assert_eq!(resolve_theme_name("catppuccin", true, false), "catppuccin");
        assert_eq!(
            resolve_theme_name("catppuccin-latin", false, true),
            "catppuccin-latin"
        );
    }

    #[test]
    fn latin_theme_entries_exist() {
        assert!(get("catppuccin-latin").is_some());
        assert!(get("catppuccin-latte-latin").is_some());
    }
}
