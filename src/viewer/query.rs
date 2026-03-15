//! Unified document query layer for viewer mode handlers.
//!
//! Bundles the four document-model parameters (`markdown`, `visual_lines`,
//! `content_index`, `content_offset`) into a single `DocumentQuery` struct
//! with a common query API.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::pipeline::ContentIndex;
use crate::tile::{self, VisualLine};

/// A URL extracted from Markdown source, with its link text.
#[derive(Debug, Clone)]
pub struct UrlEntry {
    pub url: String,
    pub text: String,
}

/// Read-only document model for viewer queries.
///
/// Bundles all document-model data that mode handlers need, providing
/// unified query methods that delegate to `tile.rs` or implement
/// viewer-specific logic.
pub struct DocumentQuery<'a> {
    pub markdown: &'a str,
    pub visual_lines: &'a [VisualLine],
    pub content_index: &'a ContentIndex,
    pub content_offset: usize,
}

impl<'a> DocumentQuery<'a> {
    pub fn new(
        markdown: &'a str,
        visual_lines: &'a [VisualLine],
        content_index: &'a ContentIndex,
        content_offset: usize,
    ) -> Self {
        Self {
            markdown,
            visual_lines,
            content_index,
            content_offset,
        }
    }

    /// Find the visual line index containing the given Markdown byte offset.
    pub fn find_visual_line_by_offset(&self, md_byte_offset: usize) -> Option<usize> {
        self.visual_lines.iter().position(|vl| {
            vl.md_block_range
                .as_ref()
                .is_some_and(|r| r.contains(&md_byte_offset))
        })
    }

    /// Find the visual line index that contains the given 1-based markdown line.
    pub fn find_visual_line_by_line(&self, md_line: usize) -> Option<usize> {
        self.visual_lines.iter().position(|vl| {
            vl.md_block_range.as_ref().is_some_and(|r| {
                let s = tile::byte_offset_to_line(self.markdown, r.start);
                let e =
                    tile::byte_offset_to_line(self.markdown, r.end.saturating_sub(1).max(r.start));
                md_line >= s && md_line <= e
            })
        })
    }

    /// Extract the precise Markdown source line for a visual line.
    ///
    /// Uses `md_offset` to locate the exact line within the block.
    /// Falls back to block-level yank when no mapping is available.
    pub fn yank_exact(&self, vl_idx: usize) -> String {
        if vl_idx >= self.visual_lines.len() {
            return String::new();
        }
        let vl = &self.visual_lines[vl_idx];
        if let Some(offset) = vl.md_offset {
            let line = tile::byte_offset_to_line(self.markdown, offset);
            self.markdown
                .lines()
                .nth(line - 1)
                .unwrap_or("")
                .to_string()
        } else {
            // Fallback to block yank (theme-derived lines)
            self.yank_lines(vl_idx, vl_idx)
        }
    }

    /// Extract Markdown source lines corresponding to a range of visual lines.
    ///
    /// Collects `md_block_range` from each visual line in `[start_vl..=end_vl]`,
    /// takes the union of all byte ranges, and returns the corresponding Markdown text.
    pub fn yank_lines(&self, start_vl: usize, end_vl: usize) -> String {
        let end_vl = end_vl.min(self.visual_lines.len().saturating_sub(1));
        if start_vl > end_vl {
            return String::new();
        }

        let mut min_offset = usize::MAX;
        let mut max_offset = 0usize;
        let mut found = false;

        for vl in &self.visual_lines[start_vl..=end_vl] {
            if let Some(ref r) = vl.md_block_range {
                min_offset = min_offset.min(r.start);
                max_offset = max_offset.max(r.end);
                found = true;
            }
        }

        if !found {
            return String::new();
        }

        let min_offset = min_offset.min(self.markdown.len());
        let max_offset = max_offset.min(self.markdown.len());
        self.markdown[min_offset..max_offset]
            .trim_end_matches('\n')
            .to_string()
    }

    /// Extract URLs from the Markdown source lines corresponding to a visual line.
    pub fn extract_urls(&self, vl_idx: usize) -> Vec<UrlEntry> {
        if vl_idx >= self.visual_lines.len() {
            return Vec::new();
        }
        let vl = &self.visual_lines[vl_idx];
        let Some(ref r) = vl.md_block_range else {
            return Vec::new();
        };
        let start = tile::byte_offset_to_line(self.markdown, r.start);
        let end = tile::byte_offset_to_line(self.markdown, r.end.saturating_sub(1).max(r.start));

        extract_urls_from_lines(self.markdown, start, end)
    }

    /// Delegate to `tile::byte_offset_to_line`.
    pub fn byte_offset_to_line(&self, offset: usize) -> usize {
        tile::byte_offset_to_line(self.markdown, offset)
    }
}

/// Extract URLs from a range of Markdown source lines (1-based, inclusive).
///
/// Step 1: Parse with pulldown-cmark to extract `[text](url)` links.
/// Step 2: Extract bare URLs (e.g., `https://example.com`) from plain text
///         using regex, deduplicating against URLs already found in step 1.
pub fn extract_urls_from_lines(md_source: &str, start: usize, end: usize) -> Vec<UrlEntry> {
    let lines: Vec<&str> = md_source.lines().collect();
    let start_idx = start.saturating_sub(1);
    let end_idx = end.min(lines.len());
    if start_idx >= lines.len() {
        return Vec::new();
    }
    let block_text = lines[start_idx..end_idx].join("\n");

    let parser = Parser::new_ext(&block_text, Options::empty());
    let mut urls = Vec::new();
    let mut in_link = false;
    let mut current_url = String::new();
    let mut current_text = String::new();
    let mut plain_texts: Vec<String> = Vec::new();
    for event in parser {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                in_link = true;
                current_url = dest_url.into_string();
                current_text.clear();
            }
            Event::End(TagEnd::Link) => {
                if in_link && !current_url.is_empty() {
                    urls.push(UrlEntry {
                        url: current_url.clone(),
                        text: current_text.clone(),
                    });
                }
                in_link = false;
            }
            Event::Text(t) if in_link => {
                current_text.push_str(&t);
            }
            Event::Code(c) if in_link => {
                current_text.push_str(&c);
            }
            Event::Text(t) => {
                plain_texts.push(t.into_string());
            }
            _ => {}
        }
    }

    for text in &plain_texts {
        for bare_url in crate::url::extract_bare_urls(text) {
            if !urls.iter().any(|u| u.url == bare_url) {
                urls.push(UrlEntry {
                    url: bare_url.clone(),
                    text: bare_url,
                });
            }
        }
    }

    urls
}

/// Shared test helpers for viewer tests.
///
/// Provides common factory functions so that each mode's `#[cfg(test)]` module
/// can `use super::query::test_helpers::*` instead of duplicating them.
#[cfg(test)]
pub(super) mod test_helpers {
    use crate::pipeline::ContentIndex;
    use crate::tile::VisualLine;

    /// Empty `ContentIndex` (no mappings).
    pub fn empty_ci() -> ContentIndex {
        ContentIndex::new(vec![], vec![])
    }

    /// One `VisualLine` per line in `md`, with `md_block_range` covering the
    /// exact byte span of each line (including trailing `\n`).
    pub fn make_visual_lines(md: &str) -> Vec<VisualLine> {
        let mut byte_offset = 0usize;
        md.lines()
            .map(|line| {
                let start = byte_offset;
                byte_offset += line.len() + 1;
                let end = byte_offset.min(md.len());
                VisualLine {
                    y_pt: 0.0,
                    y_px: 0,
                    md_block_range: Some(start..end),
                    md_offset: None,
                }
            })
            .collect()
    }

    /// Single `VisualLine` whose `md_block_range` spans the given 1-based
    /// line range in `md`. Pass `None` for a VL with no source mapping.
    pub fn make_vl(md: &str, line_range: Option<(usize, usize)>) -> VisualLine {
        let md_block_range = line_range.map(|(start_line, end_line)| {
            let start_byte: usize = md
                .split('\n')
                .take(start_line - 1)
                .map(|l| l.len() + 1)
                .sum();
            let end_byte: usize = md
                .split('\n')
                .take(end_line)
                .map(|l| l.len() + 1)
                .sum::<usize>()
                .min(md.len());
            start_byte..end_byte
        });
        VisualLine {
            y_pt: 0.0,
            y_px: 0,
            md_block_range,
            md_offset: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;

    #[test]
    fn find_visual_line_by_offset_basic() {
        let md = "hello world\nfoo bar";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);

        assert_eq!(doc.find_visual_line_by_offset(0), Some(0));
        assert_eq!(doc.find_visual_line_by_offset(5), Some(0));
        assert_eq!(doc.find_visual_line_by_offset(12), Some(1));
        assert_eq!(doc.find_visual_line_by_offset(999), None);
    }

    #[test]
    fn find_visual_line_by_line_basic() {
        let md = "# Title\n\nSome text\n";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);

        assert_eq!(doc.find_visual_line_by_line(1), Some(0));
        assert_eq!(doc.find_visual_line_by_line(3), Some(2));
    }

    #[test]
    fn yank_exact_delegates() {
        let md = "hello\nworld\n";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);

        // yank_exact requires md_offset, which our simple VLs don't have.
        // It falls back to md_block_range-based extraction.
        let result = doc.yank_exact(0);
        assert!(result.contains("hello"));
    }

    #[test]
    fn byte_offset_to_line_delegates() {
        let md = "line1\nline2\nline3\n";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);

        assert_eq!(doc.byte_offset_to_line(0), 1);
        assert_eq!(doc.byte_offset_to_line(6), 2);
        assert_eq!(doc.byte_offset_to_line(12), 3);
    }

    #[test]
    fn extract_urls_single_link() {
        let md = "Check [Rust](https://rust.invalid/) for details.\n";
        let vls = vec![make_vl(md, Some((1, 1)))];
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vls, &ci, 0);
        let urls = doc.extract_urls(0);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://rust.invalid/");
        assert_eq!(urls[0].text, "Rust");
    }

    #[test]
    fn extract_urls_multiple_links() {
        let md = "See [A](https://a.invalid/) and [B](https://b.invalid/).\n";
        let vls = vec![make_vl(md, Some((1, 1)))];
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vls, &ci, 0);
        let urls = doc.extract_urls(0);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://a.invalid/");
        assert_eq!(urls[0].text, "A");
        assert_eq!(urls[1].url, "https://b.invalid/");
        assert_eq!(urls[1].text, "B");
    }

    #[test]
    fn extract_urls_no_links() {
        let md = "Just plain text, no links here.\n";
        let vls = vec![make_vl(md, Some((1, 1)))];
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vls, &ci, 0);
        let urls = doc.extract_urls(0);
        assert!(urls.is_empty());
    }

    #[test]
    fn extract_urls_no_source_mapping() {
        let md = "Has [link](https://example.invalid/) but no mapping.\n";
        let vls = vec![make_vl(md, None)];
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vls, &ci, 0);
        let urls = doc.extract_urls(0);
        assert!(urls.is_empty());
    }

    #[test]
    fn extract_urls_out_of_bounds() {
        let md = "Some text\n";
        let vls = vec![make_vl(md, Some((1, 1)))];
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vls, &ci, 0);
        let urls = doc.extract_urls(5);
        assert!(urls.is_empty());
    }

    #[test]
    fn extract_urls_multiline_block() {
        let md = "Line 1\n[link1](https://one.invalid/)\n[link2](https://two.invalid/)\nLine 4\n";
        let vls = vec![make_vl(md, Some((2, 3)))];
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vls, &ci, 0);
        let urls = doc.extract_urls(0);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://one.invalid/");
        assert_eq!(urls[0].text, "link1");
        assert_eq!(urls[1].url, "https://two.invalid/");
        assert_eq!(urls[1].text, "link2");
    }

    #[test]
    fn extract_urls_from_lines_bare_url() {
        let md = "Check https://rust-lang.invalid/ for more\n";
        let urls = extract_urls_from_lines(md, 1, 1);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://rust-lang.invalid/");
        assert_eq!(urls[0].text, "https://rust-lang.invalid/");
    }

    #[test]
    fn extract_urls_from_lines_mixed_link_and_bare() {
        let md = "[Rust](https://rust-lang.invalid) and https://crates.invalid\n";
        let urls = extract_urls_from_lines(md, 1, 1);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://rust-lang.invalid");
        assert_eq!(urls[0].text, "Rust");
        assert_eq!(urls[1].url, "https://crates.invalid");
        assert_eq!(urls[1].text, "https://crates.invalid");
    }

    #[test]
    fn extract_urls_from_lines_bare_duplicate_with_link() {
        let md = "[Rust](https://rust-lang.invalid) and https://rust-lang.invalid\n";
        let urls = extract_urls_from_lines(md, 1, 1);
        assert_eq!(urls.len(), 1, "duplicate bare URL should be deduplicated");
        assert_eq!(urls[0].url, "https://rust-lang.invalid");
        assert_eq!(urls[0].text, "Rust");
    }

    #[test]
    fn extract_urls_from_lines_bare_urls_in_list() {
        let md = "- https://help.x.com/ja/using-x/create-a-thread\n- https://help.x.com/en/using-x/types-of-posts\n";
        let urls = extract_urls_from_lines(md, 1, 2);
        assert_eq!(urls.len(), 2, "each list item should produce one URL");
        assert_eq!(urls[0].url, "https://help.x.com/ja/using-x/create-a-thread");
        assert_eq!(urls[1].url, "https://help.x.com/en/using-x/types-of-posts");
    }
}
