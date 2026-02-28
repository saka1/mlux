use regex::Regex;
use std::sync::LazyLock;

/// Regex for bare URLs starting with `http://` or `https://`.
///
/// Pattern approach inspired by John Gruber's "liberal URL regex":
/// <https://mathiasbynens.be/demo/url-regex>
///
/// - Matches `https?://` followed by non-whitespace, non-bracket characters.
/// - Strips trailing punctuation (`. , ; : ! ? - ' "`) that is typically
///   not part of the URL but part of the surrounding prose.
static BARE_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>\)\]\}]+[^\s<>\)\]\}.,:;!?\-'"]"#).unwrap());

/// Extract bare URLs from plain text.
///
/// Finds `http://` and `https://` URLs that appear as plain text (not part
/// of markdown link syntax). The regex is intentionally simple — precision
/// is not critical since the results are presented to a human user for
/// confirmation.
pub fn extract_bare_urls(text: &str) -> Vec<String> {
    BARE_URL_RE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_url() {
        let urls = extract_bare_urls("Visit https://example.invalid for details");
        assert_eq!(urls, vec!["https://example.invalid"]);
    }

    #[test]
    fn trailing_period_excluded() {
        let urls = extract_bare_urls("See https://example.invalid/page.");
        assert_eq!(urls, vec!["https://example.invalid/page"]);
    }

    #[test]
    fn trailing_comma_excluded() {
        let urls = extract_bare_urls("https://example.invalid/page, more");
        assert_eq!(urls, vec!["https://example.invalid/page"]);
    }

    #[test]
    fn multiple_urls() {
        let urls = extract_bare_urls("https://a.invalid and https://b.invalid");
        assert_eq!(urls, vec!["https://a.invalid", "https://b.invalid"]);
    }

    #[test]
    fn path_query_fragment() {
        let urls = extract_bare_urls("https://example.invalid/path?q=1&r=2#frag");
        assert_eq!(urls, vec!["https://example.invalid/path?q=1&r=2#frag"]);
    }

    #[test]
    fn parenthesized_url() {
        let urls = extract_bare_urls("(https://example.invalid/wiki/Rust_(lang))");
        // The outer `)` is excluded by the regex character class
        assert_eq!(urls, vec!["https://example.invalid/wiki/Rust_(lang"]);
    }

    #[test]
    fn http_url() {
        let urls = extract_bare_urls("http://example.invalid");
        assert_eq!(urls, vec!["http://example.invalid"]);
    }

    #[test]
    fn no_url() {
        let urls = extract_bare_urls("plain text");
        assert!(urls.is_empty());
    }

    #[test]
    fn japanese_text() {
        let urls = extract_bare_urls("参考: https://example.invalid を見て");
        assert_eq!(urls, vec!["https://example.invalid"]);
    }

    #[test]
    fn trailing_colon_excluded() {
        let urls = extract_bare_urls("URL: https://example.invalid:");
        assert_eq!(urls, vec!["https://example.invalid"]);
    }

    #[test]
    fn trailing_exclamation_excluded() {
        let urls = extract_bare_urls("https://example.invalid!");
        assert_eq!(urls, vec!["https://example.invalid"]);
    }

    #[test]
    fn trailing_question_excluded() {
        let urls = extract_bare_urls("https://example.invalid?");
        assert_eq!(urls, vec!["https://example.invalid"]);
    }
}
