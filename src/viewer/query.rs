//! Unified document query layer for viewer mode handlers.
//!
//! Bundles the four document-model parameters (`markdown`, `visual_lines`,
//! `content_index`, `content_offset`) into a single `DocumentQuery` struct
//! with a common query API.

use crate::pipeline::ContentIndex;
use crate::tile::{self, UrlEntry, VisualLine};

/// Read-only document model for viewer queries.
///
/// Bundles all document-model data that mode handlers need, providing
/// unified query methods that delegate to `tile.rs` or implement
/// viewer-specific logic.
pub(super) struct DocumentQuery<'a> {
    pub markdown: &'a str,
    pub visual_lines: &'a [VisualLine],
    pub content_index: &'a ContentIndex,
    pub content_offset: usize,
}

impl<'a> DocumentQuery<'a> {
    pub(super) fn new(
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
    pub(super) fn find_visual_line_by_offset(&self, md_byte_offset: usize) -> Option<usize> {
        self.visual_lines.iter().position(|vl| {
            vl.md_block_range
                .as_ref()
                .is_some_and(|r| r.contains(&md_byte_offset))
        })
    }

    /// Find the visual line index that contains the given 1-based markdown line.
    pub(super) fn find_visual_line_by_line(&self, md_line: usize) -> Option<usize> {
        self.visual_lines.iter().position(|vl| {
            vl.md_block_range.as_ref().is_some_and(|r| {
                let s = tile::byte_offset_to_line(self.markdown, r.start);
                let e =
                    tile::byte_offset_to_line(self.markdown, r.end.saturating_sub(1).max(r.start));
                md_line >= s && md_line <= e
            })
        })
    }

    /// Delegate to `tile::yank_exact`.
    pub(super) fn yank_exact(&self, vl_idx: usize) -> String {
        tile::yank_exact(self.markdown, self.visual_lines, vl_idx)
    }

    /// Delegate to `tile::yank_lines`.
    pub(super) fn yank_lines(&self, start_vl: usize, end_vl: usize) -> String {
        tile::yank_lines(self.markdown, self.visual_lines, start_vl, end_vl)
    }

    /// Delegate to `tile::extract_urls`.
    pub(super) fn extract_urls(&self, vl_idx: usize) -> Vec<UrlEntry> {
        tile::extract_urls(self.markdown, self.visual_lines, vl_idx)
    }

    /// Delegate to `tile::byte_offset_to_line`.
    pub(super) fn byte_offset_to_line(&self, offset: usize) -> usize {
        tile::byte_offset_to_line(self.markdown, offset)
    }
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
}
