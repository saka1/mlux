use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use typst::foundations::Bytes;

use crate::theme::MermaidColors;

/// Compute a deterministic key from diagram source content.
pub fn diagram_key(source: &str) -> String {
    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    format!("_diagram_{:016x}.svg", h.finish())
}

/// Extract mermaid diagram blocks from Markdown.
///
/// Returns `Vec<(key, source)>` where key is a content-hash filename.
pub fn extract_diagrams(markdown: &str) -> Vec<(String, String)> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_MATH);
    let parser = Parser::new_ext(markdown, options);

    let mut diagrams = Vec::new();
    let mut in_mermaid = false;
    let mut buf = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang)))
                if lang.as_ref() == "mermaid" =>
            {
                in_mermaid = true;
                buf.clear();
            }
            Event::End(TagEnd::CodeBlock) if in_mermaid => {
                in_mermaid = false;
                let key = diagram_key(&buf);
                diagrams.push((key, std::mem::take(&mut buf)));
            }
            Event::Text(text) if in_mermaid => {
                buf.push_str(&text);
            }
            _ => {}
        }
    }

    diagrams
}

/// Fix malformed SVG from mermaid-rs-renderer.
///
/// The renderer emits unescaped double quotes inside `font-family` attributes
/// (e.g. `font-family="..., "Segoe UI", ..."`), which breaks XML parsers.
/// This replaces inner quotes with single quotes.
fn fix_svg_font_family(svg: &str) -> String {
    use regex::Regex;
    let re = Regex::new(r#"font-family="([^"]*)"#).unwrap();
    re.replace_all(svg, |caps: &regex::Captures<'_>| {
        // The first capture may be truncated if the attribute value itself
        // contains unescaped `"`. Walk forward from the match start to find the
        // real closing `"` by counting balanced quotes in the font-family value.
        // However, a simpler approach: find the full attribute span.
        let _ = caps;
        // Fallback: handled below
        caps[0].to_string()
    })
    .to_string();

    // Simpler approach: find `font-family="` and fix the value
    let needle = "font-family=\"";
    let mut result = String::with_capacity(svg.len());
    let mut pos = 0;
    while let Some(start) = svg[pos..].find(needle) {
        let abs_start = pos + start;
        let val_start = abs_start + needle.len();

        // Find the closing quote: after `font-family="`, look for `" ` or `"/>`
        // to handle the case where internal quotes exist.
        // Heuristic: the attribute value ends at `"` followed by ` `, `>`, or `/`.
        let mut end = val_start;
        loop {
            match svg[end..].find('"') {
                Some(offset) => {
                    let q_pos = end + offset;
                    let next_ch = svg[q_pos + 1..].chars().next();
                    if matches!(next_ch, Some(' ' | '>' | '/') | None) {
                        // This is the real closing quote
                        result.push_str(&svg[pos..val_start]);
                        let val = &svg[val_start..q_pos];
                        result.push_str(&val.replace('"', "'"));
                        pos = q_pos;
                        break;
                    }
                    end = q_pos + 1;
                }
                None => {
                    // No closing quote found; emit as-is
                    result.push_str(&svg[pos..val_start]);
                    pos = val_start;
                    break;
                }
            }
        }
    }
    result.push_str(&svg[pos..]);
    result
}

/// Build `RenderOptions` from theme-provided Mermaid colours.
fn mermaid_options(colors: &MermaidColors) -> mermaid_rs_renderer::RenderOptions {
    let mut opts = mermaid_rs_renderer::RenderOptions::modern();
    let t = &mut opts.theme;
    t.background = colors.background.into();
    t.primary_color = colors.primary_color.into();
    t.secondary_color = colors.secondary_color.into();
    t.tertiary_color = colors.tertiary_color.into();
    t.primary_text_color = colors.primary_text_color.into();
    t.text_color = colors.text_color.into();
    t.primary_border_color = colors.primary_border_color.into();
    t.line_color = colors.line_color.into();
    t.edge_label_background = colors.edge_label_background.into();
    t.cluster_background = colors.cluster_background.into();
    t.cluster_border = colors.cluster_border.into();
    t.sequence_actor_fill = colors.sequence_actor_fill.into();
    t.sequence_actor_border = colors.sequence_actor_border.into();
    t.sequence_actor_line = colors.sequence_actor_line.into();
    t.sequence_note_fill = colors.sequence_note_fill.into();
    t.sequence_note_border = colors.sequence_note_border.into();
    t.sequence_activation_fill = colors.sequence_activation_fill.into();
    t.sequence_activation_border = colors.sequence_activation_border.into();
    opts
}

/// Render diagram blocks to SVG bytes.
///
/// Uses `catch_unwind` to handle panics from the renderer gracefully.
/// Failed diagrams are logged and omitted from the result.
pub fn render_diagrams(
    diagrams: &[(String, String)],
    colors: &MermaidColors,
) -> Vec<(String, Bytes)> {
    let opts = mermaid_options(colors);
    diagrams
        .iter()
        .filter_map(|(key, source)| {
            let opts = opts.clone();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                mermaid_rs_renderer::render_with_options(source, opts)
            })) {
                Ok(Ok(svg)) => {
                    let fixed = fix_svg_font_family(&svg);
                    Some((key.clone(), Bytes::new(fixed.into_bytes())))
                }
                Ok(Err(e)) => {
                    log::warn!("diagram {key}: {e}");
                    None
                }
                Err(_) => {
                    log::warn!("diagram {key}: renderer panicked");
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagram_key_deterministic() {
        let src = "graph LR\n  A --> B";
        assert_eq!(diagram_key(src), diagram_key(src));
    }

    #[test]
    fn test_diagram_key_different_sources() {
        assert_ne!(
            diagram_key("graph LR\n  A --> B"),
            diagram_key("graph TD\n  A --> B")
        );
    }

    #[test]
    fn test_extract_diagrams_finds_mermaid() {
        let md = "# Title\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nSome text.\n";
        let diagrams = extract_diagrams(md);
        assert_eq!(diagrams.len(), 1);
        assert!(diagrams[0].1.contains("graph LR"));
    }

    #[test]
    fn test_extract_diagrams_ignores_other_code_blocks() {
        let md = "```rust\nfn main() {}\n```\n\n```python\nprint('hello')\n```\n";
        let diagrams = extract_diagrams(md);
        assert!(diagrams.is_empty());
    }

    #[test]
    fn test_extract_diagrams_multiple() {
        let md = "```mermaid\ngraph LR\n  A --> B\n```\n\n```mermaid\ngraph TD\n  C --> D\n```\n";
        let diagrams = extract_diagrams(md);
        assert_eq!(diagrams.len(), 2);
    }

    fn light_colors() -> &'static MermaidColors {
        crate::theme::mermaid_colors("catppuccin-latte")
    }

    #[test]
    fn test_render_diagrams_valid() {
        let diagrams = vec![("test.svg".to_string(), "graph LR\n  A --> B".to_string())];
        let results = render_diagrams(&diagrams, light_colors());
        assert_eq!(results.len(), 1);
        let svg = std::str::from_utf8(results[0].1.as_slice()).unwrap();
        assert!(
            svg.contains("<svg"),
            "should produce SVG output, got: {}",
            &svg[..svg.len().min(200)]
        );
    }

    #[test]
    fn test_fix_svg_font_family() {
        let bad = r#"font-family="Inter, "Segoe UI", sans-serif""#;
        let fixed = fix_svg_font_family(bad);
        assert_eq!(fixed, r#"font-family="Inter, 'Segoe UI', sans-serif""#);
    }

    #[test]
    fn test_fix_svg_font_family_no_inner_quotes() {
        let good = r#"font-family="Inter, sans-serif""#;
        let fixed = fix_svg_font_family(good);
        assert_eq!(fixed, good);
    }

    #[test]
    fn test_render_diagrams_invalid_graceful() {
        let diagrams = vec![(
            "bad.svg".to_string(),
            "not a valid diagram at all %%%".to_string(),
        )];
        let results = render_diagrams(&diagrams, light_colors());
        // Should not panic; may produce empty or may still render
        // The important thing is no panic
        let _ = results;
    }
}
