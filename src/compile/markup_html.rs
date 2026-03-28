use std::collections::HashSet;

use regex::Regex;

use super::markup_util;

/// Extract `src` attribute values from `<img>` tags in an HTML fragment.
///
/// Handles both double-quoted and single-quoted attribute values.
/// Case-insensitive for the tag name and attribute name.
pub fn extract_img_srcs(html: &str) -> Vec<String> {
    let re = Regex::new(r#"(?i)<img\b[^>]*\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap();
    let mut srcs = Vec::new();
    for caps in re.captures_iter(html) {
        let src = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        if !src.is_empty() {
            srcs.push(src);
        }
    }
    srcs
}

/// Render `<img>` tags in an HTML fragment as Typst snippets.
///
/// For each `<img src="...">` found:
/// - If `src` is in `available_images`, emit `#align(center)[#image("path")]`
/// - Otherwise, emit a placeholder block
///
/// Returns an empty Vec if no `<img>` tags are found.
pub fn render_html_imgs(html: &str, available_images: Option<&HashSet<String>>) -> Vec<String> {
    let srcs = extract_img_srcs(html);
    let mut snippets = Vec::new();
    for src in srcs {
        let is_available = available_images.is_some_and(|set| set.contains(&src));
        if is_available {
            snippets.push(markup_util::typst_image(&src));
        } else {
            snippets.push(markup_util::typst_image_placeholder(&src));
        }
    }
    snippets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_basic() {
        assert_eq!(
            extract_img_srcs(r#"<img src="photo.png">"#),
            vec!["photo.png"]
        );
    }

    #[test]
    fn extract_with_attributes() {
        assert_eq!(
            extract_img_srcs(r#"<img src="photo.png" alt="x" width="700">"#),
            vec!["photo.png"]
        );
    }

    #[test]
    fn extract_single_quote() {
        assert_eq!(extract_img_srcs("<img src='photo.png'>"), vec!["photo.png"]);
    }

    #[test]
    fn extract_case_insensitive() {
        assert_eq!(
            extract_img_srcs(r#"<IMG SRC="photo.png">"#),
            vec!["photo.png"]
        );
    }

    #[test]
    fn extract_no_src() {
        assert!(extract_img_srcs(r#"<img alt="x">"#).is_empty());
    }

    #[test]
    fn extract_multiple() {
        let html = r#"<img src="a.png"><img src="b.png">"#;
        assert_eq!(extract_img_srcs(html), vec!["a.png", "b.png"]);
    }

    #[test]
    fn extract_non_img_tag() {
        assert!(extract_img_srcs(r#"<div><a href="foo.png"></a></div>"#).is_empty());
    }

    #[test]
    fn extract_empty_src() {
        assert!(extract_img_srcs(r#"<img src="">"#).is_empty());
    }

    #[test]
    fn render_available() {
        let available: HashSet<String> = ["photo.png".to_string()].into_iter().collect();
        let result = render_html_imgs(r#"<img src="photo.png">"#, Some(&available));
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("#image(\"photo.png\")"));
    }

    #[test]
    fn render_unavailable() {
        let available: HashSet<String> = HashSet::new();
        let result = render_html_imgs(r#"<img src="missing.png">"#, Some(&available));
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("#image-placeholder("));
    }

    #[test]
    fn render_no_img() {
        let result = render_html_imgs("<div>hello</div>", None);
        assert!(result.is_empty());
    }
}
