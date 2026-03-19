use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Classified link destination, determined at URL extraction time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// External URL — open in browser (https://, http://, mailto:, etc.)
    ExternalUrl(String),
    /// Local markdown file — relative path as extracted, not yet resolved.
    LocalMarkdown(String),
}

impl LinkTarget {
    /// Classify a URL string into external or local markdown.
    pub fn classify(url: &str) -> Self {
        if url.contains("://") || url.starts_with("mailto:") {
            return Self::ExternalUrl(url.to_string());
        }
        let path_part = url.split('#').next().unwrap_or(url);
        if path_part.ends_with(".md") || path_part.ends_with(".markdown") {
            Self::LocalMarkdown(url.to_string())
        } else {
            Self::ExternalUrl(url.to_string())
        }
    }

    /// Extract the inner URL string for display.
    pub fn display_url(&self) -> &str {
        match self {
            Self::ExternalUrl(u) | Self::LocalMarkdown(u) => u,
        }
    }
}

/// Resolve a relative link URL against the current file's directory.
pub fn resolve_link_path(url: &str, current_file: &Path) -> Option<PathBuf> {
    let path_part = url.split('#').next().unwrap_or(url);
    if path_part.is_empty() {
        return None;
    }
    let base_dir = current_file.parent()?;
    Some(base_dir.join(path_part))
}

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

    #[test]
    fn classify_local_markdown() {
        assert_eq!(
            LinkTarget::classify("./other.md"),
            LinkTarget::LocalMarkdown("./other.md".into())
        );
        assert_eq!(
            LinkTarget::classify("other.md"),
            LinkTarget::LocalMarkdown("other.md".into())
        );
        assert_eq!(
            LinkTarget::classify("../docs/guide.md"),
            LinkTarget::LocalMarkdown("../docs/guide.md".into())
        );
        assert_eq!(
            LinkTarget::classify("file.md#section"),
            LinkTarget::LocalMarkdown("file.md#section".into())
        );
        assert_eq!(
            LinkTarget::classify("notes.markdown"),
            LinkTarget::LocalMarkdown("notes.markdown".into())
        );
    }

    #[test]
    fn classify_external() {
        assert_eq!(
            LinkTarget::classify("https://example.com"),
            LinkTarget::ExternalUrl("https://example.com".into())
        );
        assert_eq!(
            LinkTarget::classify("http://example.com/page.md"),
            LinkTarget::ExternalUrl("http://example.com/page.md".into())
        );
        assert_eq!(
            LinkTarget::classify("mailto:user@example.com"),
            LinkTarget::ExternalUrl("mailto:user@example.com".into())
        );
        assert_eq!(
            LinkTarget::classify("data.csv"),
            LinkTarget::ExternalUrl("data.csv".into())
        );
        assert_eq!(
            LinkTarget::classify("image.png"),
            LinkTarget::ExternalUrl("image.png".into())
        );
        assert_eq!(LinkTarget::classify(""), LinkTarget::ExternalUrl("".into()));
    }

    #[test]
    fn test_resolve_link_path() {
        let current = Path::new("/home/user/docs/readme.md");

        assert_eq!(
            resolve_link_path("other.md", current),
            Some(PathBuf::from("/home/user/docs/other.md"))
        );
        assert_eq!(
            resolve_link_path("../guide.md", current),
            Some(PathBuf::from("/home/user/docs/../guide.md"))
        );
        assert_eq!(
            resolve_link_path("sub/page.md#heading", current),
            Some(PathBuf::from("/home/user/docs/sub/page.md"))
        );
        // Fragment-only link -> None
        assert_eq!(resolve_link_path("#heading", current), None);
        // Empty -> None
        assert_eq!(resolve_link_path("", current), None);
    }

    #[test]
    fn display_url_returns_inner() {
        let ext = LinkTarget::ExternalUrl("https://example.com".into());
        assert_eq!(ext.display_url(), "https://example.com");
        let local = LinkTarget::LocalMarkdown("other.md".into());
        assert_eq!(local.display_url(), "other.md");
    }
}
