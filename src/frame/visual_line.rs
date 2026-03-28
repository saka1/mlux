use std::ops::Range;
use std::time::Instant;

use log::{debug, info, trace};
use serde::{Deserialize, Serialize};
use typst::introspection::Tag;
use typst::layout::{Frame, FrameItem, PagedDocument, Point};
use typst::syntax::Span;

use crate::compile::BoundIndex;

/// Compute pixel size matching typst_render's formula exactly.
///
/// typst_render uses `(pixel_per_pt: f32 * size.to_f32()).round().max(1.0) as u32`.
/// We must use the same f32 arithmetic + round() to avoid off-by-one mismatches
/// (ceil on f64 can produce values 1px larger than the actual rendered image).
pub(crate) fn pt_to_px(pt: f64, ppi: f32) -> u32 {
    let pixel_per_pt = ppi / 72.0;
    (pixel_per_pt * pt as f32).round().max(1.0) as u32
}

/// A visual line extracted from the PagedDocument frame tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualLine {
    pub y_pt: f64, // Absolute Y coordinate of the text baseline (pt)
    pub y_px: u32, // Pixel Y coordinate (after ppi conversion)
    /// Byte range of the Markdown block this visual line belongs to.
    /// None for theme-derived text (e.g., list markers from set rules).
    pub md_block_range: Option<Range<usize>>,
    /// Precise Markdown byte offset of the text at this visual line's position.
    /// Always Some when md_block_range is Some.
    pub md_offset: Option<usize>,
}

/// Extract visual line positions from the frame tree (without source mapping).
///
/// Compatibility wrapper — delegates to `extract_visual_lines_with_map` with
/// no source mapping, producing `md_block_range = None` for all lines.
pub fn extract_visual_lines(document: &PagedDocument, ppi: f32) -> Vec<VisualLine> {
    extract_visual_lines_with_map(document, ppi, None)
}

/// Extract visual line positions from the frame tree, optionally with source mapping.
///
/// Walks all TextItem nodes, collects their absolute Y coordinates and (optionally)
/// representative Span from the first glyph. Deduplicates with tolerance, then
/// resolves each line's Span through the `BoundIndex` to get the corresponding
/// Markdown byte position and block range.
pub fn extract_visual_lines_with_map(
    document: &PagedDocument,
    ppi: f32,
    mapping: Option<&BoundIndex<'_>>,
) -> Vec<VisualLine> {
    let start = Instant::now();
    if document.pages.is_empty() {
        return Vec::new();
    }

    // Collect visual lines using frame tree structure.
    //
    // Instead of flattening all TextItems and grouping by Y tolerance (which
    // breaks on math with superscripts/subscripts), we use the Frame tree's
    // Group boundaries to determine visual lines. Groups that contain line
    // structure (lists, tables, code blocks) are recursed into; leaf Groups
    // (paragraphs, headings, display math) become single visual lines.
    let mut raw_lines: Vec<(f64, Vec<Span>)> = Vec::new();
    collect_visual_lines_structural(&document.pages[0].frame, Point::zero(), &mut raw_lines);
    raw_lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Merge lines at the same Y position (tolerance 5pt) to combine
    // bare Text items (e.g., bullet markers) with adjacent Group lines.
    let mut deduped: Vec<(f64, Vec<Span>)> = Vec::new();
    for (y, spans) in raw_lines {
        if deduped
            .last()
            .is_none_or(|(prev_y, _)| (y - prev_y).abs() > 5.0)
        {
            deduped.push((y, spans));
        } else if let Some(last) = deduped.last_mut() {
            last.1.extend(spans);
        }
    }

    let lines: Vec<VisualLine> = deduped
        .into_iter()
        .enumerate()
        .map(|(i, (y_pt, spans))| {
            trace!(
                "visual_line[{i}]: y={y_pt:.1}pt, {} span candidates",
                spans.len()
            );
            // Try each span candidate until one resolves to a content-area line.
            // This handles cases where theme-derived spans (e.g., list markers
            // from `#set list(marker: ...)`) coexist with content spans on the
            // same visual line. The order of spans depends on frame tree traversal
            // and is not guaranteed, so we try all candidates rather than relying
            // on position.
            let pos = mapping.and_then(|bi| {
                spans
                    .iter()
                    .filter(|s| !s.is_detached())
                    .find_map(|&s| bi.resolve_span(s))
            });
            VisualLine {
                y_pt,
                y_px: pt_to_px(y_pt, ppi),
                md_block_range: pos.as_ref().map(|p| p.block_range.clone()),
                md_offset: pos.as_ref().map(|p| p.offset),
            }
        })
        .collect();

    info!(
        "visual_line: extract completed in {:.1}ms ({} lines)",
        start.elapsed().as_secs_f64() * 1000.0,
        lines.len()
    );
    if let Some(bi) = mapping {
        let mapped = lines.iter().filter(|l| l.md_block_range.is_some()).count();
        debug!(
            "extract_visual_lines: {} lines ({} mapped, {} unmapped)",
            lines.len(),
            mapped,
            lines.len() - mapped
        );
        for (i, vl) in lines.iter().enumerate() {
            if let Some(ref r) = vl.md_block_range {
                let s = byte_offset_to_line(bi.md_source(), r.start);
                let e = byte_offset_to_line(bi.md_source(), r.end.saturating_sub(1).max(r.start));
                let preview: String = bi
                    .md_source()
                    .lines()
                    .nth(s - 1)
                    .unwrap_or("")
                    .chars()
                    .take(60)
                    .collect();
                debug!("  vl[{i}] y={:.1}pt → md L{s}-{e}: {:?}", vl.y_pt, preview);
            } else {
                debug!("  vl[{i}] y={:.1}pt → (unmapped)", vl.y_pt);
            }
        }
    }

    lines
}

/// Convert a byte offset within a string to a 1-based line number.
pub fn byte_offset_to_line(source: &str, offset: usize) -> usize {
    let offset = offset.min(source.len());
    source[..offset].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Collect visual lines from a frame using its tree structure.
///
/// Instead of flattening all TextItems, this uses Group boundaries to determine
/// visual lines. A Group is either:
/// - Recursed into (if it has line structure or a dominant child Group)
/// - Emitted as a single visual line (leaf Group: paragraphs, headings, math)
///
/// Bare Text items (not inside a Group at this level) are collected into a
/// pending buffer and flushed with Y-tolerance grouping.
fn collect_visual_lines_structural(
    frame: &Frame,
    parent_offset: Point,
    out: &mut Vec<(f64, Vec<Span>)>,
) {
    let mut pending_texts: Vec<(f64, Span)> = Vec::new();

    for (pos, item) in frame.items() {
        let abs = parent_offset + *pos;
        match item {
            FrameItem::Tag(_) | FrameItem::Link(_, _) | FrameItem::Shape(_, _) => {}
            FrameItem::Image(_, _, _) => {}
            FrameItem::Text(text) => {
                if let Some(span) = text.glyphs.first().map(|g| g.span.0) {
                    pending_texts.push((abs.y.to_pt(), span));
                }
            }
            FrameItem::Group(group) => {
                if should_recurse(&group.frame) {
                    // Flush pending texts before recursing
                    flush_pending_texts(&mut pending_texts, out);
                    collect_visual_lines_structural(&group.frame, abs, out);
                } else {
                    // Flush pending texts before emitting group line
                    flush_pending_texts(&mut pending_texts, out);
                    // Emit the entire Group as one visual line
                    let baseline_y = find_representative_baseline(&group.frame, abs);
                    let spans = collect_all_spans_recursive(&group.frame);
                    if !spans.is_empty() {
                        out.push((baseline_y, spans));
                    }
                }
            }
        }
    }

    // Flush any remaining pending texts
    flush_pending_texts(&mut pending_texts, out);
}

/// Check whether a Group frame should be recursed into for visual line extraction.
fn should_recurse(frame: &Frame) -> bool {
    has_line_structure(frame) || has_dominant_child_group(frame) || has_raw_line_tags(frame)
}

/// Check if child Groups are arranged vertically without overlap (line structure).
///
/// Returns true when there are 2+ child Groups arranged top-to-bottom,
/// indicating the Group contains multiple visual lines (e.g., lists, tables).
fn has_line_structure(frame: &Frame) -> bool {
    let mut child_groups: Vec<(f64, f64)> = Vec::new(); // (y, h)
    for (pos, item) in frame.items() {
        if let FrameItem::Group(g) = item {
            child_groups.push((pos.y.to_pt(), g.frame.size().y.to_pt()));
        }
    }
    if child_groups.len() < 2 {
        return false;
    }
    child_groups.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    // Check that each child Group starts at or after the previous one ends (1pt tolerance)
    child_groups
        .windows(2)
        .all(|w| w[1].0 >= w[0].0 + w[0].1 - 1.0)
}

/// Check if any child Group occupies more than 50% of the parent's height.
///
/// This catches containers like code blocks where a single large child Group
/// holds the actual line content.
fn has_dominant_child_group(frame: &Frame) -> bool {
    let parent_h = frame.size().y.to_pt();
    if parent_h <= 0.0 {
        return false;
    }
    frame.items().any(|(_, item)| {
        if let FrameItem::Group(g) = item {
            g.frame.size().y.to_pt() > parent_h * 0.5
        } else {
            false
        }
    })
}

/// Check if the frame contains RawLine Tags (code block line markers).
///
/// Typst's `RawLine` element is `Tagged`, so every code block line —
/// even for unrecognized languages — emits `Tag::Start`/`Tag::End` pairs.
/// When syntax highlighting is active, each line also gets a child Group;
/// when it's not, only bare Text items remain. This function detects
/// the latter case so that `should_recurse` can split them into
/// individual visual lines.
fn has_raw_line_tags(frame: &Frame) -> bool {
    frame.items().any(|(_, item)| {
        if let FrameItem::Tag(Tag::Start(content, _)) = item {
            content.elem().name() == "line"
        } else {
            false
        }
    })
}

/// Find the Y coordinate of the first TextItem in a Group (used as baseline).
fn find_representative_baseline(frame: &Frame, offset: Point) -> f64 {
    for (pos, item) in frame.items() {
        let abs_y = offset.y.to_pt() + pos.y.to_pt();
        match item {
            FrameItem::Text(_) => return abs_y,
            FrameItem::Group(g) => {
                let child_offset = Point::new(offset.x + pos.x, offset.y + pos.y);
                // Recurse to find first text
                let result = find_representative_baseline_inner(&g.frame, child_offset);
                if let Some(y) = result {
                    return y;
                }
            }
            _ => {}
        }
    }
    // Fallback: use the offset Y itself
    offset.y.to_pt()
}

/// Inner recursive helper for find_representative_baseline.
fn find_representative_baseline_inner(frame: &Frame, offset: Point) -> Option<f64> {
    for (pos, item) in frame.items() {
        let abs_y = offset.y.to_pt() + pos.y.to_pt();
        match item {
            FrameItem::Text(_) => return Some(abs_y),
            FrameItem::Group(g) => {
                let child_offset = Point::new(offset.x + pos.x, offset.y + pos.y);
                if let Some(y) = find_representative_baseline_inner(&g.frame, child_offset) {
                    return Some(y);
                }
            }
            _ => {}
        }
    }
    None
}

/// Recursively collect all Spans from TextItems within a Group.
fn collect_all_spans_recursive(frame: &Frame) -> Vec<Span> {
    let mut spans = Vec::new();
    collect_spans_inner(frame, &mut spans);
    spans
}

fn collect_spans_inner(frame: &Frame, out: &mut Vec<Span>) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Text(text) => {
                if let Some(span) = text.glyphs.first().map(|g| g.span.0) {
                    out.push(span);
                }
            }
            FrameItem::Group(g) => {
                collect_spans_inner(&g.frame, out);
            }
            _ => {}
        }
    }
}

/// Flush pending bare Text items into visual lines, grouping by Y with 5pt tolerance.
fn flush_pending_texts(pending: &mut Vec<(f64, Span)>, out: &mut Vec<(f64, Vec<Span>)>) {
    if pending.is_empty() {
        return;
    }
    pending.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for &(y, span) in pending.iter() {
        if out
            .last()
            .is_none_or(|(prev_y, _)| (y - prev_y).abs() > 5.0)
        {
            out.push((y, vec![span]));
        } else if let Some(last) = out.last_mut() {
            last.1.push(span);
        }
    }
    pending.clear();
}
