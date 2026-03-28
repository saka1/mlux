use std::ops::Range;

use serde::{Deserialize, Serialize};
use typst::syntax::{Source, Span};

use super::markup_util::is_typst_escapable;

/// Markdown source line → Typst output byte range mapping for a single block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMapping {
    /// Byte range within the Typst output (content_text).
    pub typst_byte_range: Range<usize>,
    /// Byte range within the original Markdown source.
    pub md_byte_range: Range<usize>,
}

/// Kind of a text span within the Typst output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanKind {
    /// Plain text (Typst-escaped from Markdown plain text).
    Plain,
    /// Inline code or code block content (1:1 byte mapping with Markdown).
    Code,
    /// Math expression (inline or display).
    Math,
    /// Soft/hard break.
    Break,
    /// Opaque content (images, mermaid diagrams) — not searchable.
    Opaque,
}

/// A span of text in the Typst output, mapped back to the Markdown source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSpan {
    /// Byte range within the content_text (Typst output of markdown_to_typst).
    pub typst_range: Range<usize>,
    /// Byte range within the original Markdown source.
    pub md_range: Range<usize>,
    /// Kind of content this span represents.
    pub kind: SpanKind,
}

/// Bidirectional index between Markdown source and Typst output.
///
/// Built during `markdown_to_typst()`.
/// Used by the highlight system to map Markdown regex matches to
/// main.typ byte ranges for glyph-level highlighting, and by the yank
/// system via `BoundIndex` to resolve Spans to Markdown positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentIndex {
    text_spans: Vec<TextSpan>,
    block_spans: Vec<BlockMapping>,
}

impl ContentIndex {
    pub fn new(text_spans: Vec<TextSpan>, block_spans: Vec<BlockMapping>) -> Self {
        Self {
            text_spans,
            block_spans,
        }
    }

    pub fn text_spans(&self) -> &[TextSpan] {
        &self.text_spans
    }

    pub fn block_spans(&self) -> &[BlockMapping] {
        &self.block_spans
    }

    /// Find the TextSpan whose typst_range contains `typst_offset`.
    fn find_text_span_by_typst_offset(&self, typst_offset: usize) -> Option<&TextSpan> {
        let idx = self
            .text_spans
            .binary_search_by(|s| {
                if typst_offset < s.typst_range.start {
                    std::cmp::Ordering::Greater
                } else if typst_offset >= s.typst_range.end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.text_spans[idx])
    }

    /// Find the BlockMapping whose typst_byte_range contains `typst_offset`.
    fn find_block_by_typst_offset(&self, typst_offset: usize) -> Option<&BlockMapping> {
        let idx = self
            .block_spans
            .binary_search_by(|b| {
                if typst_offset < b.typst_byte_range.start {
                    std::cmp::Ordering::Greater
                } else if typst_offset >= b.typst_byte_range.end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.block_spans[idx])
    }

    /// Convert Markdown byte ranges to main.typ byte ranges for highlighting.
    ///
    /// Each input range is a match within the Markdown source. This method
    /// finds overlapping TextSpans and converts the Markdown byte offsets
    /// to Typst byte offsets, applying escape corrections for Plain text.
    ///
    /// `content_offset` is added to convert from content_text-local offsets
    /// to main.typ-absolute offsets (where the Typst Source object lives).
    pub fn md_to_main_ranges(
        &self,
        md_ranges: &[Range<usize>],
        md_source: &str,
        content_offset: usize,
    ) -> Vec<Range<usize>> {
        let mut result: Vec<Range<usize>> = Vec::new();

        for md_range in md_ranges {
            for span in &self.text_spans {
                // Check overlap
                if span.md_range.start >= md_range.end || span.md_range.end <= md_range.start {
                    continue;
                }

                match span.kind {
                    SpanKind::Break | SpanKind::Opaque => continue,
                    SpanKind::Math => {
                        // Math: return the entire typst_range (partial match not meaningful)
                        let start = span.typst_range.start + content_offset;
                        let end = span.typst_range.end + content_offset;
                        result.push(start..end);
                    }
                    SpanKind::Code => {
                        // Code: 1:1 byte mapping between MD and Typst content
                        let overlap_start = md_range.start.max(span.md_range.start);
                        let overlap_end = md_range.end.min(span.md_range.end);
                        let local_start = overlap_start - span.md_range.start;
                        let local_end = overlap_end - span.md_range.start;
                        let typst_start = span.typst_range.start + local_start + content_offset;
                        let typst_end = span.typst_range.start + local_end + content_offset;
                        result.push(typst_start..typst_end);
                    }
                    SpanKind::Plain => {
                        // Plain: need escape correction
                        let overlap_start = md_range.start.max(span.md_range.start);
                        let overlap_end = md_range.end.min(span.md_range.end);
                        let md_text = &md_source[span.md_range.clone()];
                        let local_start = overlap_start - span.md_range.start;
                        let local_end = overlap_end - span.md_range.start;
                        let typst_local_start = md_to_typst_local(md_text, local_start);
                        let typst_local_end = md_to_typst_local(md_text, local_end);
                        let typst_start =
                            span.typst_range.start + typst_local_start + content_offset;
                        let typst_end = span.typst_range.start + typst_local_end + content_offset;
                        result.push(typst_start..typst_end);
                    }
                }
            }
        }

        // Merge adjacent/overlapping ranges
        merge_ranges(&mut result);

        result
    }
}

/// Markdown byte position resolved from a Typst span.
pub struct MdPosition {
    /// Byte offset within the Markdown source.
    pub offset: usize,
    /// Byte range of the containing Markdown block.
    pub block_range: Range<usize>,
}

/// Wraps a `ContentIndex` with the context needed to resolve Typst `Span`s
/// to Markdown byte positions.
///
/// Used by the yank system to map rendered glyph spans back to source lines.
pub struct BoundIndex<'a> {
    index: &'a ContentIndex,
    source: &'a Source,
    content_offset: usize,
    md_source: &'a str,
}

impl<'a> BoundIndex<'a> {
    pub fn new(
        index: &'a ContentIndex,
        source: &'a Source,
        content_offset: usize,
        md_source: &'a str,
    ) -> Self {
        Self {
            index,
            source,
            content_offset,
            md_source,
        }
    }

    pub fn md_source(&self) -> &str {
        self.md_source
    }

    /// Resolve a Typst `Span` to a Markdown byte position.
    ///
    /// Returns `None` for detached spans and spans that fall within the
    /// theme/prefix region (before `content_offset`) rather than content.
    pub fn resolve_span(&self, span: Span) -> Option<MdPosition> {
        if span.is_detached() {
            return None;
        }
        let main_range = self.source.range(span)?;
        if main_range.start < self.content_offset {
            return None; // Within theme prefix, not content
        }
        let typst_offset = main_range.start - self.content_offset;

        // Block lookup is required — no block = not a content position
        let block = self.index.find_block_by_typst_offset(typst_offset)?;

        // Text span lookup is optional — improves precision within the block
        let md_offset = match self.index.find_text_span_by_typst_offset(typst_offset) {
            Some(ts) => {
                let typst_local = typst_offset - ts.typst_range.start;
                match ts.kind {
                    SpanKind::Code => ts.md_range.start + typst_local,
                    SpanKind::Plain => {
                        let md_text = &self.md_source[ts.md_range.clone()];
                        ts.md_range.start + typst_to_md_local(md_text, typst_local)
                    }
                    SpanKind::Math | SpanKind::Break | SpanKind::Opaque => ts.md_range.start,
                }
            }
            None => block.md_byte_range.start,
        };

        Some(MdPosition {
            offset: md_offset,
            block_range: block.md_byte_range.clone(),
        })
    }
}

/// Convert a byte offset within Markdown plain text to the corresponding
/// byte offset within the Typst-escaped version of the same text.
///
/// Typst escaping adds a `\` before each escapable character, so each
/// such character occupies 2 bytes in the output instead of 1.
fn md_to_typst_local(md_text: &str, md_offset: usize) -> usize {
    let mut typst_offset = 0;
    for (i, ch) in md_text.char_indices() {
        if i >= md_offset {
            break;
        }
        if is_typst_escapable(ch) {
            typst_offset += 1 + ch.len_utf8(); // backslash + char
        } else {
            typst_offset += ch.len_utf8();
        }
    }
    typst_offset
}

/// Convert a byte offset within the Typst-escaped text back to the
/// corresponding byte offset in the original Markdown plain text.
///
/// This is the inverse of `md_to_typst_local`.
fn typst_to_md_local(md_text: &str, typst_local: usize) -> usize {
    let mut typst_pos = 0usize;
    for (i, ch) in md_text.char_indices() {
        if typst_pos >= typst_local {
            return i;
        }
        typst_pos += if is_typst_escapable(ch) {
            1 + ch.len_utf8() // backslash + char
        } else {
            ch.len_utf8()
        };
    }
    md_text.len()
}

/// Convert a rendered byte offset (as stored in glyph.span.1) back to
/// the source byte offset, accounting for Typst `\X` escape sequences
/// where 2 source bytes produce 1 rendered byte.
pub fn rendered_to_source_byte(source_text: &str, rendered_offset: usize) -> usize {
    let mut rendered = 0usize;
    let bytes = source_text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if rendered >= rendered_offset {
            return i;
        }
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            // Escape sequence: `\X` in source → single char in rendered
            let next_ch_len = char_len_at(bytes, i + 1);
            rendered += next_ch_len;
            i += 1 + next_ch_len; // skip backslash + escaped char
        } else {
            let ch_len = char_len_at(bytes, i);
            rendered += ch_len;
            i += ch_len;
        }
    }
    i
}

/// Length of the UTF-8 character starting at `bytes[pos]`.
fn char_len_at(bytes: &[u8], pos: usize) -> usize {
    if pos >= bytes.len() {
        return 1;
    }
    let b = bytes[pos];
    if b < 0x80 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

/// Sort and merge overlapping or adjacent ranges in place.
fn merge_ranges(ranges: &mut Vec<Range<usize>>) {
    if ranges.len() <= 1 {
        return;
    }
    ranges.sort_by_key(|r| r.start);
    let mut write = 0;
    for read in 1..ranges.len() {
        if ranges[read].start <= ranges[write].end {
            // Overlap or adjacent — extend
            ranges[write].end = ranges[write].end.max(ranges[read].end);
        } else {
            write += 1;
            ranges[write] = ranges[read].clone();
        }
    }
    ranges.truncate(write + 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- md_to_typst_local tests ---

    #[test]
    fn md_to_typst_local_no_escapes() {
        assert_eq!(md_to_typst_local("hello", 0), 0);
        assert_eq!(md_to_typst_local("hello", 3), 3);
        assert_eq!(md_to_typst_local("hello", 5), 5);
    }

    #[test]
    fn md_to_typst_local_with_escapes() {
        // "#hello" → "\#hello"
        // md offset 0 → typst offset 0 (before #)
        assert_eq!(md_to_typst_local("#hello", 0), 0);
        // md offset 1 → typst offset 2 (after \#)
        assert_eq!(md_to_typst_local("#hello", 1), 2);
        // md offset 3 → typst offset 4 (after \#he)
        assert_eq!(md_to_typst_local("#hello", 3), 4);
    }

    #[test]
    fn md_to_typst_local_multibyte_utf8() {
        // "日本語" — no escapes, 3 bytes per char
        assert_eq!(md_to_typst_local("日本語", 0), 0);
        assert_eq!(md_to_typst_local("日本語", 3), 3); // after 日
        assert_eq!(md_to_typst_local("日本語", 6), 6); // after 本
    }

    #[test]
    fn md_to_typst_local_mixed() {
        // "$100" → "\$100"
        assert_eq!(md_to_typst_local("$100", 0), 0);
        assert_eq!(md_to_typst_local("$100", 1), 2); // after \$
        assert_eq!(md_to_typst_local("$100", 4), 5); // after \$100
    }

    // --- typst_to_md_local tests ---

    #[test]
    fn typst_to_md_local_no_escapes() {
        assert_eq!(typst_to_md_local("hello", 0), 0);
        assert_eq!(typst_to_md_local("hello", 3), 3);
        assert_eq!(typst_to_md_local("hello", 5), 5);
    }

    #[test]
    fn typst_to_md_local_with_escapes() {
        // MD: "#hello", Typst: "\#hello"
        // typst offset 0 → md offset 0 (before #)
        assert_eq!(typst_to_md_local("#hello", 0), 0);
        // typst offset 2 → md offset 1 (after \#, pointing to 'h')
        assert_eq!(typst_to_md_local("#hello", 2), 1);
        // typst offset 4 → md offset 3 (after \#he)
        assert_eq!(typst_to_md_local("#hello", 4), 3);
    }

    #[test]
    fn typst_to_md_local_mixed() {
        // MD: "$100", Typst: "\$100"
        assert_eq!(typst_to_md_local("$100", 0), 0);
        assert_eq!(typst_to_md_local("$100", 2), 1); // after \$
        assert_eq!(typst_to_md_local("$100", 5), 4); // end
    }

    #[test]
    fn typst_to_md_local_roundtrip() {
        // Forward then back should recover the original md_offset
        let text = "#hello $world";
        for md_off in 0..=text.len() {
            // Only roundtrip at char boundaries
            if !text.is_char_boundary(md_off) {
                continue;
            }
            let typst = md_to_typst_local(text, md_off);
            let recovered = typst_to_md_local(text, typst);
            assert_eq!(
                recovered, md_off,
                "roundtrip failed at md_off={md_off}: typst={typst}, recovered={recovered}"
            );
        }
    }

    // --- rendered_to_source_byte tests ---

    #[test]
    fn rendered_to_source_no_escapes() {
        assert_eq!(rendered_to_source_byte("hello", 0), 0);
        assert_eq!(rendered_to_source_byte("hello", 3), 3);
        assert_eq!(rendered_to_source_byte("hello", 5), 5);
    }

    #[test]
    fn rendered_to_source_with_escape() {
        // Source: "\#hello" (7 bytes), rendered: "#hello" (6 bytes)
        assert_eq!(rendered_to_source_byte("\\#hello", 0), 0);
        // rendered offset 1 → source offset 2 (after \#)
        assert_eq!(rendered_to_source_byte("\\#hello", 1), 2);
        assert_eq!(rendered_to_source_byte("\\#hello", 3), 4);
    }

    #[test]
    fn rendered_to_source_double_backslash() {
        // Source: "\\\\world" = \\world (7 bytes), rendered: "\world" (6 bytes)
        // Actually in Typst source, \\ means escaped backslash
        assert_eq!(rendered_to_source_byte("\\\\world", 0), 0);
        assert_eq!(rendered_to_source_byte("\\\\world", 1), 2);
    }

    #[test]
    fn rendered_to_source_combined() {
        // "\#a\\b" → rendered "#a\b" (4 bytes), source is 6 bytes
        assert_eq!(rendered_to_source_byte("\\#a\\\\b", 0), 0);
        assert_eq!(rendered_to_source_byte("\\#a\\\\b", 1), 2); // after \#
        assert_eq!(rendered_to_source_byte("\\#a\\\\b", 2), 3); // after \#a
        assert_eq!(rendered_to_source_byte("\\#a\\\\b", 3), 5); // after \#a\\
    }

    // --- md_to_main_ranges tests ---

    #[test]
    fn md_to_main_ranges_plain() {
        let ci = ContentIndex::new(
            vec![TextSpan {
                typst_range: 0..5, // "hello"
                md_range: 0..5,
                kind: SpanKind::Plain,
            }],
            vec![],
        );
        let result = ci.md_to_main_ranges(&[1..3], "hello", 100);
        // "el" at md 1..3 → typst 1..3 + 100 = 101..103
        assert_eq!(result, vec![101..103]);
    }

    #[test]
    fn md_to_main_ranges_plain_with_escape() {
        // MD: "#hi" → Typst: "\#hi"
        let ci = ContentIndex::new(
            vec![TextSpan {
                typst_range: 0..4, // "\#hi"
                md_range: 0..3,    // "#hi"
                kind: SpanKind::Plain,
            }],
            vec![],
        );
        // Search for "hi" at md 1..3
        let result = ci.md_to_main_ranges(&[1..3], "#hi", 10);
        // md offset 1 → typst local 2, md offset 3 → typst local 4
        // → 2+10..4+10 = 12..14
        assert_eq!(result, vec![12..14]);
    }

    #[test]
    fn md_to_main_ranges_code() {
        let ci = ContentIndex::new(
            vec![TextSpan {
                typst_range: 10..15, // code content
                md_range: 5..10,
                kind: SpanKind::Code,
            }],
            vec![],
        );
        let result = ci.md_to_main_ranges(&[6..9], "xxxxx12345", 0);
        // overlap: 6..9 in md_range 5..10 → local 1..4 → typst 11..14
        assert_eq!(result, vec![11..14]);
    }

    #[test]
    fn md_to_main_ranges_math() {
        let ci = ContentIndex::new(
            vec![TextSpan {
                typst_range: 0..20, // entire math expression
                md_range: 0..10,
                kind: SpanKind::Math,
            }],
            vec![],
        );
        // Any overlap → return entire typst_range
        let result = ci.md_to_main_ranges(&[3..5], "0123456789", 50);
        assert_eq!(result, vec![50..70]);
    }

    #[test]
    fn md_to_main_ranges_opaque_skipped() {
        let ci = ContentIndex::new(
            vec![TextSpan {
                typst_range: 0..30,
                md_range: 0..10,
                kind: SpanKind::Opaque,
            }],
            vec![],
        );
        let result = ci.md_to_main_ranges(&[0..10], "0123456789", 0);
        assert!(result.is_empty());
    }

    #[test]
    fn md_to_main_ranges_merge_adjacent() {
        let ci = ContentIndex::new(
            vec![
                TextSpan {
                    typst_range: 0..5,
                    md_range: 0..5,
                    kind: SpanKind::Plain,
                },
                TextSpan {
                    typst_range: 5..10,
                    md_range: 5..10,
                    kind: SpanKind::Plain,
                },
            ],
            vec![],
        );
        // Search across both spans
        let result = ci.md_to_main_ranges(&[3..8], "0123456789", 0);
        // Should merge into a single range: 3..8
        assert_eq!(result, vec![3..8]);
    }

    // --- merge_ranges tests ---

    #[test]
    fn merge_ranges_empty() {
        let mut r: Vec<Range<usize>> = vec![];
        merge_ranges(&mut r);
        assert!(r.is_empty());
    }

    #[test]
    fn merge_ranges_overlapping() {
        let mut r = vec![1..5, 3..8, 10..15];
        merge_ranges(&mut r);
        assert_eq!(r, vec![1..8, 10..15]);
    }

    #[test]
    fn merge_ranges_adjacent() {
        let mut r = vec![1..5, 5..10];
        merge_ranges(&mut r);
        assert_eq!(r, vec![1..10]);
    }
}
