/// Whether a character has special meaning in Typst markup and needs escaping.
pub(super) fn is_typst_escapable(ch: char) -> bool {
    matches!(
        ch,
        '#' | '*' | '_' | '`' | '<' | '>' | '@' | '$' | '\\' | '/' | '~' | '(' | ')' | '[' | ']'
    )
}

/// Escape characters that have special meaning in Typst markup.
pub(super) fn escape_typst(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if is_typst_escapable(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Escape characters meaningful inside a Typst string literal (`"..."`).
pub(super) fn escape_typst_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

/// Render an image path as a Typst `#image()` call (centered).
pub(super) fn typst_image(path: &str) -> String {
    let escaped = escape_typst_string_literal(path);
    format!("#align(center)[#image(\"{escaped}\")]\n")
}

/// Render a placeholder block for an unavailable image.
///
/// Calls the `image-placeholder` function defined in the theme, so each theme
/// can use its own palette color (e.g. Catppuccin Surface 2).
pub(super) fn typst_image_placeholder(path: &str) -> String {
    let escaped = escape_typst_string_literal(path);
    format!("#image-placeholder(\"{escaped}\")\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_special_chars() {
        assert_eq!(escape_typst("#hello"), "\\#hello");
        assert_eq!(escape_typst("a * b"), "a \\* b");
        assert_eq!(escape_typst("$100"), "\\$100");
        assert_eq!(escape_typst("foo(bar)"), "foo\\(bar\\)");
        assert_eq!(escape_typst("foo[bar]"), "foo\\[bar\\]");
    }

    #[test]
    fn test_escape_typst_string_literal_backslash() {
        assert_eq!(escape_typst_string_literal("foo\\bar"), "foo\\\\bar");
    }

    #[test]
    fn test_escape_typst_string_literal_quote() {
        assert_eq!(escape_typst_string_literal("foo\"bar"), "foo\\\"bar");
    }

    #[test]
    fn test_escape_typst_string_literal_both() {
        assert_eq!(escape_typst_string_literal("a\\\"b"), "a\\\\\\\"b");
    }

    #[test]
    fn test_typst_image() {
        assert_eq!(
            typst_image("photo.png"),
            "#align(center)[#image(\"photo.png\")]\n"
        );
    }

    #[test]
    fn test_typst_image_with_special_chars() {
        assert_eq!(
            typst_image("path\\to\"img.png"),
            "#align(center)[#image(\"path\\\\to\\\"img.png\")]\n"
        );
    }

    #[test]
    fn test_typst_image_placeholder() {
        let result = typst_image_placeholder("missing.png");
        assert!(result.contains("#image-placeholder("), "got: {result}");
        assert!(result.contains("missing.png"), "got: {result}");
    }
}
