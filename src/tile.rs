use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Instant;

use anyhow::{Result, bail};
use log::{debug, info, trace};
use serde::{Deserialize, Serialize};
use typst::foundations::Smart;
use typst::layout::{Abs, Axes, Frame, FrameItem, PagedDocument, Point};
use typst::syntax::{Source, Span};
use typst::visualize::{Geometry, Paint};

use crate::pipeline::SourceMap;

/// Compute pixel size matching typst_render's formula exactly.
///
/// typst_render uses `(pixel_per_pt: f32 * size.to_f32()).round().max(1.0) as u32`.
/// We must use the same f32 arithmetic + round() to avoid off-by-one mismatches
/// (ceil on f64 can produce values 1px larger than the actual rendered image).
fn pt_to_px(pt: f64, ppi: f32) -> u32 {
    let pixel_per_pt = ppi / 72.0;
    (pixel_per_pt * pt as f32).round().max(1.0) as u32
}

/// A visual line extracted from the PagedDocument frame tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualLine {
    pub y_pt: f64, // Absolute Y coordinate of the text baseline (pt)
    pub y_px: u32, // Pixel Y coordinate (after ppi conversion)
    /// Markdown source line range (1-based, inclusive). None for theme-derived text.
    pub md_line_range: Option<(usize, usize)>,
    /// Precise 1-based MD line (y yank). Set for code blocks and any block where
    /// Typst/MD newline counts match (lists, paragraphs, headings). None when
    /// line-level resolution is unsafe (tables, nested blockquotes).
    pub md_line_exact: Option<usize>,
}

/// Result of resolving a Span to Markdown line information.
struct MdLineInfo {
    range: (usize, usize),
    exact: Option<usize>,
}

/// Extract visual line positions from the frame tree (without source mapping).
///
/// Compatibility wrapper — delegates to `extract_visual_lines_with_map` with
/// no source mapping, producing `md_line_range = None` for all lines.
pub fn extract_visual_lines(document: &PagedDocument, ppi: f32) -> Vec<VisualLine> {
    extract_visual_lines_with_map(document, ppi, None)
}

/// Parameters for source mapping during visual line extraction.
pub struct SourceMappingParams<'a> {
    pub source: &'a Source,
    pub content_offset: usize,
    pub source_map: &'a SourceMap,
    pub md_source: &'a str,
}

/// Extract visual line positions from the frame tree, optionally with source mapping.
///
/// Walks all TextItem nodes, collects their absolute Y coordinates and (optionally)
/// representative Span from the first glyph. Deduplicates with tolerance, then
/// resolves each line's Span through the source mapping chain to get the
/// corresponding Markdown source line range.
pub fn extract_visual_lines_with_map(
    document: &PagedDocument,
    ppi: f32,
    mapping: Option<&SourceMappingParams>,
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
            let info = mapping.and_then(|m| {
                spans
                    .iter()
                    .filter(|s| !s.is_detached())
                    .find_map(|&s| resolve_md_line_range(s, m))
            });
            VisualLine {
                y_pt,
                y_px: pt_to_px(y_pt, ppi),
                md_line_range: info.as_ref().map(|i| i.range),
                md_line_exact: info.as_ref().and_then(|i| i.exact),
            }
        })
        .collect();

    info!(
        "tile: extract_visual_lines completed in {:.1}ms ({} lines)",
        start.elapsed().as_secs_f64() * 1000.0,
        lines.len()
    );
    if let Some(m) = mapping {
        let mapped = lines.iter().filter(|l| l.md_line_range.is_some()).count();
        debug!(
            "extract_visual_lines: {} lines ({} mapped, {} unmapped)",
            lines.len(),
            mapped,
            lines.len() - mapped
        );
        for (i, vl) in lines.iter().enumerate() {
            if let Some((s, e)) = vl.md_line_range {
                let preview: String = m
                    .md_source
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

/// Resolve a Span to a Markdown line range via the source mapping chain.
///
/// Chain: Span → Source::range() → subtract content_offset → SourceMap lookup → md line range
///
/// Also computes `exact`: the precise 1-based MD line corresponding to this
/// span position within the block. For code blocks, uses a fence-aware offset
/// (+1 to skip opening fence). For other blocks, uses newline counting when
/// Typst and MD blocks have matching newline counts (safety check). Blocks that
/// fail the check (tables, nested blockquotes) get `exact = None`.
fn resolve_md_line_range(span: Span, params: &SourceMappingParams) -> Option<MdLineInfo> {
    if span.is_detached() {
        trace!("  span detached, skipping");
        return None;
    }

    // Resolve Span to byte range in main.typ
    let main_range = params.source.range(span)?;

    // Convert to content_text offset
    if main_range.start < params.content_offset {
        trace!(
            "  span in prefix (main_range={:?}, content_offset={})",
            main_range, params.content_offset
        );
        return None; // Within theme/prefix, not content
    }
    let content_offset = main_range.start - params.content_offset;

    // Look up in SourceMap
    let block = params.source_map.find_by_typst_offset(content_offset)?;

    // Convert md_byte_range to line numbers (1-based)
    let start_line = byte_offset_to_line(params.md_source, block.md_byte_range.start);
    let end_line = byte_offset_to_line(
        params.md_source,
        block
            .md_byte_range
            .end
            .saturating_sub(1)
            .max(block.md_byte_range.start),
    );

    // Compute exact line within the block using newline counting.
    //
    // For code blocks (fenced with "```"): skip the opening fence line (+1)
    // and clamp to exclude the closing fence.
    //
    // For other blocks: use the same newline counting, but only when the Typst
    // and MD block texts have matching newline counts (1:1 line correspondence).
    // This works for lists, paragraphs, headings. Tables and nested blockquotes
    // have different line structures and safely fall back to None.
    // See docs/line-exact-generalization.md for detailed analysis.
    let md_block_text = &params.md_source[block.md_byte_range.clone()];
    let typst_local_offset = content_offset - block.typst_byte_range.start;
    let typst_block_text = params.source.text().get(
        (block.typst_byte_range.start + params.content_offset)
            ..(block.typst_byte_range.end + params.content_offset),
    );

    let exact = if let Some(typst_text) = typst_block_text {
        let is_code_block = md_block_text.starts_with("```");
        let clamped = typst_local_offset.min(typst_text.len());
        let newlines_before = typst_text[..clamped]
            .bytes()
            .filter(|&b| b == b'\n')
            .count();

        if is_code_block {
            // start_line is the "```" fence line; content starts at start_line + 1
            let exact_line = start_line + 1 + newlines_before;
            // Clamp to not exceed end_line - 1 (closing fence)
            let exact_line = exact_line
                .min(end_line.saturating_sub(1))
                .max(start_line + 1);
            trace!(
                "  code block exact: typst_local_off={}, newlines={}, exact_line={}",
                typst_local_offset, newlines_before, exact_line
            );
            Some(exact_line)
        } else {
            // Safety check: only compute exact when line structure is preserved.
            let md_newlines = md_block_text.bytes().filter(|&b| b == b'\n').count();
            let typst_newlines = typst_text.bytes().filter(|&b| b == b'\n').count();
            if md_newlines == typst_newlines {
                let exact_line = (start_line + newlines_before).clamp(start_line, end_line);
                trace!(
                    "  generic exact: typst_local_off={}, newlines={}, exact_line={} (md_nl={}, typst_nl={})",
                    typst_local_offset, newlines_before, exact_line, md_newlines, typst_newlines
                );
                Some(exact_line)
            } else {
                trace!(
                    "  newline mismatch: md_nl={}, typst_nl={} — skipping exact",
                    md_newlines, typst_newlines
                );
                None
            }
        }
    } else {
        None
    };

    trace!(
        "  span resolved: main={:?} → content_off={} → typst_block={:?} → md_block={:?} → lines {}-{} exact={:?}",
        main_range,
        content_offset,
        block.typst_byte_range,
        block.md_byte_range,
        start_line,
        end_line,
        exact
    );

    Some(MdLineInfo {
        range: (start_line, end_line),
        exact,
    })
}

/// Convert a byte offset within a string to a 1-based line number.
fn byte_offset_to_line(source: &str, offset: usize) -> usize {
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
    has_line_structure(frame) || has_dominant_child_group(frame)
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

/// Extract Markdown source lines corresponding to a range of visual lines.
///
/// Collects `md_line_range` from each visual line in `[start_vl..=end_vl]`,
/// takes the union of all ranges, and returns the corresponding Markdown lines.
pub fn yank_lines(
    md_source: &str,
    visual_lines: &[VisualLine],
    start_vl: usize,
    end_vl: usize,
) -> String {
    let end_vl = end_vl.min(visual_lines.len().saturating_sub(1));
    if start_vl > end_vl {
        return String::new();
    }

    // Collect all md_line_range values from the selected visual lines
    let mut min_line = usize::MAX;
    let mut max_line = 0usize;
    let mut found = false;

    for vl in &visual_lines[start_vl..=end_vl] {
        if let Some((start, end)) = vl.md_line_range {
            min_line = min_line.min(start);
            max_line = max_line.max(end);
            found = true;
        }
    }

    if !found {
        return String::new();
    }

    // Extract lines min_line..=max_line (1-based) from md_source
    let lines: Vec<&str> = md_source.lines().collect();
    let start_idx = min_line.saturating_sub(1); // Convert to 0-based
    let end_idx = max_line.min(lines.len()); // 1-based end → exclusive 0-based

    if start_idx >= lines.len() {
        return String::new();
    }

    lines[start_idx..end_idx].join("\n")
}

/// Extract the precise Markdown source line for a visual line.
///
/// Returns the single line indicated by `md_line_exact` when available
/// (code blocks, lists, paragraphs, headings — any block where Typst/MD
/// line structure is 1:1). Falls back to block-level yank via `yank_lines`
/// for blocks where per-line resolution is unsafe (tables, nested blockquotes).
pub fn yank_exact(md_source: &str, visual_lines: &[VisualLine], vl_idx: usize) -> String {
    if vl_idx >= visual_lines.len() {
        return String::new();
    }
    let vl = &visual_lines[vl_idx];
    if let Some(exact_line) = vl.md_line_exact {
        // Return the single exact line (1-based)
        md_source
            .lines()
            .nth(exact_line - 1)
            .unwrap_or("")
            .to_string()
    } else {
        // Fallback to block yank
        yank_lines(md_source, visual_lines, vl_idx, vl_idx)
    }
}

/// A URL extracted from Markdown source, with its link text.
#[derive(Debug, Clone)]
pub struct UrlEntry {
    pub url: String,
    pub text: String,
}

/// Extract URLs from the Markdown source lines corresponding to a visual line.
///
/// Uses `md_line_range` to find the relevant Markdown source lines, then parses
/// them with pulldown-cmark to extract link destination URLs and link text.
pub fn extract_urls(md_source: &str, visual_lines: &[VisualLine], vl_idx: usize) -> Vec<UrlEntry> {
    if vl_idx >= visual_lines.len() {
        return Vec::new();
    }
    let vl = &visual_lines[vl_idx];
    let Some((start, end)) = vl.md_line_range else {
        return Vec::new();
    };

    extract_urls_from_lines(md_source, start, end)
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

    // Step 1: Parse with pulldown-cmark and collect link URLs + text.
    // Also collect individual plain text fragments for bare URL extraction.
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
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

    // Step 2: Extract bare URLs from each text fragment independently,
    // deduplicating against URLs already found in step 1.
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

// ---------------------------------------------------------------------------
// Frame splitting — split a tall frame into vertical tiles
// ---------------------------------------------------------------------------

/// Estimate the bounding height of a FrameItem in pt.
fn item_bounding_height(item: &FrameItem) -> f64 {
    match item {
        FrameItem::Group(g) => g.frame.size().y.to_pt(),
        FrameItem::Text(t) => t.size.to_pt(),
        FrameItem::Shape(shape, _) => match &shape.geometry {
            Geometry::Line(p) => p.y.to_pt().abs(),
            Geometry::Rect(size) => size.y.to_pt(),
            // Conservative: use curve bounding box height (max - min Y)
            Geometry::Curve(curve) => {
                let mut min_y = f64::MAX;
                let mut max_y = f64::MIN;
                for item in curve.0.iter() {
                    let y = match item {
                        typst::visualize::CurveItem::Move(p) => p.y.to_pt(),
                        typst::visualize::CurveItem::Line(p) => p.y.to_pt(),
                        typst::visualize::CurveItem::Cubic(p1, p2, p3) => {
                            // Take max of all control/end points
                            let ys = [p1.y.to_pt(), p2.y.to_pt(), p3.y.to_pt()];
                            min_y = min_y.min(ys[0]).min(ys[1]).min(ys[2]);
                            ys.into_iter()
                                .max_by(|a, b| a.partial_cmp(b).unwrap())
                                .unwrap()
                        }
                        typst::visualize::CurveItem::Close => continue,
                    };
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
                if max_y > min_y { max_y - min_y } else { 0.0 }
            }
        },
        FrameItem::Image(_, size, _) => size.y.to_pt(),
        FrameItem::Link(_, size) => size.y.to_pt(),
        FrameItem::Tag(_) => 0.0,
    }
}

/// Split a compiled Frame into vertical tiles of `tile_height_pt` each.
///
/// Items spanning a tile boundary are cloned into both tiles.
/// tiny-skia clips drawing outside the canvas, so the visual result is correct.
pub fn split_frame(frame: &Frame, tile_height_pt: f64) -> Vec<Frame> {
    let start = Instant::now();
    let total_height = frame.size().y.to_pt();
    let tile_count = (total_height / tile_height_pt).ceil().max(1.0) as usize;
    let orig_width = frame.size().x;

    let mut tiles = Vec::with_capacity(tile_count);

    for i in 0..tile_count {
        let y_start = i as f64 * tile_height_pt;
        let y_end = ((i + 1) as f64 * tile_height_pt).min(total_height);
        let tile_h = y_end - y_start;

        let mut sub = Frame::hard(Axes {
            x: orig_width,
            y: Abs::pt(tile_h),
        });

        let mut item_count = 0u32;
        let mut spanning_count = 0u32;

        for (pos, item) in frame.items() {
            let item_y = pos.y.to_pt();
            let item_h = item_bounding_height(item);
            let item_bottom = item_y + item_h;

            // Does item overlap [y_start, y_end)?
            if item_bottom > y_start && item_y < y_end {
                let new_pos = Point::new(pos.x, Abs::pt(item_y - y_start));
                sub.push(new_pos, item.clone());
                item_count += 1;

                // Check if item spans beyond this tile
                if item_y < y_start || item_bottom > y_end {
                    spanning_count += 1;
                }
            }
        }

        debug!(
            "tile {}: {} items, {} boundary-spanning",
            i, item_count, spanning_count
        );
        tiles.push(sub);
    }

    info!(
        "tile: split_frame completed in {:.1}ms ({} tiles, height={}pt)",
        start.elapsed().as_secs_f64() * 1000.0,
        tile_count,
        tile_height_pt
    );
    tiles
}

// ---------------------------------------------------------------------------
// TileHash — content-addressed tile identity
// ---------------------------------------------------------------------------

/// 64-bit content hash identifying a tile's visual content.
///
/// Uses `Frame`'s derive `Hash` (which covers all fields including Span).
/// Same source text + same `FileId` produces identical Spans, so the hash
/// is deterministic across recompilations of the same input.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TileHash(u64);

/// Combined hash of content + sidebar tiles. Both must match for cache reuse.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TilePairHash {
    pub content: TileHash,
    pub sidebar: TileHash,
}

/// Hash a Frame using its derive `Hash` impl.
pub fn compute_tile_hash(frame: &Frame) -> TileHash {
    let mut h = DefaultHasher::new();
    frame.hash(&mut h);
    TileHash(h.finish())
}

// ---------------------------------------------------------------------------
// TiledDocument — lazy tile-based rendering
// ---------------------------------------------------------------------------

/// Which tiles are visible for a given scroll position.
#[derive(Debug)]
pub enum VisibleTiles {
    /// Viewport fits entirely within one tile.
    Single { idx: usize, src_y: u32, src_h: u32 },
    /// Viewport straddles two tiles.
    Split {
        top_idx: usize,
        top_src_y: u32,
        top_src_h: u32,
        bot_idx: usize,
        bot_src_h: u32,
    },
}

/// Pure metadata extracted from a [`TiledDocument`].
///
/// Contains all information needed for layout, scrolling, and visual line queries
/// without requiring access to the renderable tile data. Serializable for IPC.
#[derive(Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub tile_count: usize,
    pub width_px: u32,
    pub sidebar_width_px: u32,
    pub tile_height_px: u32,
    pub total_height_px: u32,
    pub page_height_pt: f64,
    pub visual_lines: Vec<VisualLine>,
    /// Per-tile content hashes (tile index order). Empty if not computed.
    #[serde(default)]
    pub tile_hashes: Vec<TilePairHash>,
}

impl DocumentMeta {
    /// Determine which tile(s) are visible at a given scroll offset.
    pub fn visible_tiles(&self, global_y: u32, vp_h: u32) -> VisibleTiles {
        let top_tile = (global_y / self.tile_height_px) as usize;
        let top_tile = top_tile.min(self.tile_count.saturating_sub(1));
        let src_y_in_tile = global_y - (top_tile as u32 * self.tile_height_px);
        // For the last tile, actual height may be less than tile_height_px.
        // Use total_height_px to derive actual height of each tile.
        let tile_actual_h = self.tile_actual_height_px(top_tile);
        let remaining_in_top = tile_actual_h.saturating_sub(src_y_in_tile);

        if remaining_in_top >= vp_h || top_tile + 1 >= self.tile_count {
            let src_h = vp_h.min(remaining_in_top);
            debug!(
                "display: single tile {}, src_y={}, src_h={}, vp_h={}",
                top_tile, src_y_in_tile, src_h, vp_h
            );
            VisibleTiles::Single {
                idx: top_tile,
                src_y: src_y_in_tile,
                src_h,
            }
        } else {
            let top_src_h = remaining_in_top;
            let bot_idx = top_tile + 1;
            let bot_src_h = (vp_h - top_src_h).min(self.tile_actual_height_px(bot_idx));
            debug!(
                "display: split tiles [{}, {}], top_src_y={}, top_h={}, bot_h={}, vp_h={}",
                top_tile, bot_idx, src_y_in_tile, top_src_h, bot_src_h, vp_h
            );
            VisibleTiles::Split {
                top_idx: top_tile,
                top_src_y: src_y_in_tile,
                top_src_h,
                bot_idx,
                bot_src_h,
            }
        }
    }

    /// Maximum scroll offset.
    pub fn max_scroll(&self, vp_h: u32) -> u32 {
        self.total_height_px.saturating_sub(vp_h)
    }

    /// Snap a global_y to the nearest visual line boundary.
    pub fn snap_to_line(&self, global_y: u32) -> u32 {
        if self.visual_lines.is_empty() {
            return global_y;
        }
        let mut best = self.visual_lines[0].y_px;
        let mut best_dist = (global_y as i64 - best as i64).unsigned_abs();
        for vl in &self.visual_lines {
            let dist = (global_y as i64 - vl.y_px as i64).unsigned_abs();
            if dist < best_dist {
                best = vl.y_px;
                best_dist = dist;
            }
        }
        best
    }

    /// Actual pixel height of a specific tile (last tile may be shorter).
    fn tile_actual_height_px(&self, idx: usize) -> u32 {
        if idx + 1 < self.tile_count {
            self.tile_height_px
        } else {
            // Last tile: remaining height
            self.total_height_px
                .saturating_sub(idx as u32 * self.tile_height_px)
        }
    }
}

/// A document split into renderable tiles for lazy, bounded-memory rendering.
///
/// All methods take `&self` — rendering is pure (no internal caching).
/// Use [`TiledDocumentCache`] separately for caching rendered PNGs.
pub struct TiledDocument {
    tiles: Vec<Frame>,
    sidebar_tiles: Vec<Frame>,
    sidebar_fill: Smart<Option<Paint>>,
    page_fill: Smart<Option<Paint>>,
    ppi: f32,
    width_px: u32,
    sidebar_width_px: u32,
    tile_height_px: u32,
    total_height_px: u32,
    page_height_pt: f64,
    visual_lines: Vec<VisualLine>,
}

impl TiledDocument {
    /// Build a TiledDocument from a compiled content + sidebar PagedDocument.
    ///
    /// - `sidebar_doc`: compiled sidebar document (same page height as content)
    /// - `visual_lines`: pre-extracted visual line positions (avoids redundant extraction)
    /// - `tile_height_pt`: height of each tile in typst points
    /// - `ppi`: rendering resolution
    pub fn new(
        document: &PagedDocument,
        sidebar_doc: &PagedDocument,
        visual_lines: Vec<VisualLine>,
        tile_height_pt: f64,
        ppi: f32,
    ) -> Result<Self> {
        if document.pages.is_empty() {
            bail!("[BUG] document has no pages");
        }
        let page = &document.pages[0];

        let page_size = page.frame.size();
        info!(
            "compiled: {:.1}x{:.1}pt, {} top-level items",
            page_size.x.to_pt(),
            page_size.y.to_pt(),
            page.frame.items().count()
        );

        let tiles = split_frame(&page.frame, tile_height_pt);

        // Split sidebar with the same tile boundaries
        if sidebar_doc.pages.is_empty() {
            bail!("[BUG] sidebar document has no pages");
        }
        let sidebar_page = &sidebar_doc.pages[0];
        let sidebar_tiles = split_frame(&sidebar_page.frame, tile_height_pt);
        let sidebar_width_px = pt_to_px(sidebar_page.frame.size().x.to_pt(), ppi);
        info!(
            "sidebar: {} tiles, {}px wide",
            sidebar_tiles.len(),
            sidebar_width_px
        );

        let width_px = pt_to_px(page_size.x.to_pt(), ppi);
        let tile_height_px = pt_to_px(tile_height_pt, ppi);
        let total_height_px = pt_to_px(page_size.y.to_pt(), ppi);
        let page_height_pt = page_size.y.to_pt();

        Ok(Self {
            tiles,
            sidebar_tiles,
            sidebar_fill: sidebar_page.fill.clone(),
            page_fill: page.fill.clone(),
            ppi,
            width_px,
            sidebar_width_px,
            tile_height_px,
            total_height_px,
            page_height_pt,
            visual_lines,
        })
    }

    /// Number of tiles.
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Document width in pixels.
    pub fn width_px(&self) -> u32 {
        self.width_px
    }

    /// Sidebar width in pixels.
    pub fn sidebar_width_px(&self) -> u32 {
        self.sidebar_width_px
    }

    /// Height of one standard tile in pixels.
    pub fn tile_height_px(&self) -> u32 {
        self.tile_height_px
    }

    /// Total document height in pixels.
    pub fn total_height_px(&self) -> u32 {
        self.total_height_px
    }

    /// Page height in typst points (for sidebar generation).
    pub fn page_height_pt(&self) -> f64 {
        self.page_height_pt
    }

    /// Actual pixel height of a specific tile (last tile may be shorter).
    fn tile_actual_height_px(&self, idx: usize) -> u32 {
        pt_to_px(self.tiles[idx].size().y.to_pt(), self.ppi)
    }

    /// Render a single content tile to PNG bytes.
    ///
    /// This is a pure function -- no internal caching.
    /// Thread-safe: `Frame.items` uses `Arc`, `typst_render::render` is stateless.
    pub fn render_tile(&self, idx: usize) -> Result<Vec<u8>> {
        self.render_frame(idx, &self.tiles, &self.page_fill, "content")
    }

    /// Render a single sidebar tile to PNG bytes.
    pub fn render_sidebar_tile(&self, idx: usize) -> Result<Vec<u8>> {
        self.render_frame(idx, &self.sidebar_tiles, &self.sidebar_fill, "sidebar")
    }

    /// Render a frame from `tiles` at `idx` to PNG bytes.
    fn render_frame(
        &self,
        idx: usize,
        tiles: &[Frame],
        fill: &Smart<Option<Paint>>,
        label: &str,
    ) -> Result<Vec<u8>> {
        assert!(idx < tiles.len(), "{label} tile index out of bounds");
        trace!("rendering {label} tile {idx}");
        crate::pipeline::render_frame_to_png(&tiles[idx], fill, self.ppi)
    }

    /// Determine which tile(s) are visible at a given scroll offset.
    pub fn visible_tiles(&self, global_y: u32, vp_h: u32) -> VisibleTiles {
        let top_tile = (global_y / self.tile_height_px) as usize;
        let top_tile = top_tile.min(self.tiles.len().saturating_sub(1));
        let src_y_in_tile = global_y - (top_tile as u32 * self.tile_height_px);
        let top_actual_h = self.tile_actual_height_px(top_tile);
        let remaining_in_top = top_actual_h.saturating_sub(src_y_in_tile);

        if remaining_in_top >= vp_h || top_tile + 1 >= self.tiles.len() {
            // Viewport fits in one tile (or no more tiles)
            let src_h = vp_h.min(remaining_in_top);
            debug!(
                "display: single tile {}, src_y={}, src_h={}, vp_h={}",
                top_tile, src_y_in_tile, src_h, vp_h
            );
            VisibleTiles::Single {
                idx: top_tile,
                src_y: src_y_in_tile,
                src_h,
            }
        } else {
            // Viewport straddles two tiles
            let top_src_h = remaining_in_top;
            let bot_idx = top_tile + 1;
            let bot_src_h = (vp_h - top_src_h).min(self.tile_actual_height_px(bot_idx));
            debug!(
                "display: split tiles [{}, {}], top_src_y={}, top_h={}, bot_h={}, vp_h={}",
                top_tile, bot_idx, src_y_in_tile, top_src_h, bot_src_h, vp_h
            );
            VisibleTiles::Split {
                top_idx: top_tile,
                top_src_y: src_y_in_tile,
                top_src_h,
                bot_idx,
                bot_src_h,
            }
        }
    }

    /// Maximum scroll offset.
    pub fn max_scroll(&self, vp_h: u32) -> u32 {
        self.total_height_px.saturating_sub(vp_h)
    }

    /// Snap a global_y to the nearest visual line boundary.
    pub fn snap_to_line(&self, global_y: u32) -> u32 {
        if self.visual_lines.is_empty() {
            return global_y;
        }
        let mut best = self.visual_lines[0].y_px;
        let mut best_dist = (global_y as i64 - best as i64).unsigned_abs();
        for vl in &self.visual_lines {
            let dist = (global_y as i64 - vl.y_px as i64).unsigned_abs();
            if dist < best_dist {
                best = vl.y_px;
                best_dist = dist;
            }
        }
        best
    }

    /// Access visual lines.
    pub fn visual_lines(&self) -> &[VisualLine] {
        &self.visual_lines
    }

    /// Compute content hashes for all tile pairs.
    pub fn compute_tile_hashes(&self) -> Vec<TilePairHash> {
        let start = Instant::now();
        let hashes: Vec<TilePairHash> = self
            .tiles
            .iter()
            .zip(self.sidebar_tiles.iter())
            .map(|(content, sidebar)| TilePairHash {
                content: compute_tile_hash(content),
                sidebar: compute_tile_hash(sidebar),
            })
            .collect();
        info!(
            "tile: computed {} tile hashes in {:.1}ms",
            hashes.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        hashes
    }

    /// Extract pure metadata (no renderable data), including tile hashes.
    pub fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            tile_count: self.tiles.len(),
            width_px: self.width_px,
            sidebar_width_px: self.sidebar_width_px,
            tile_height_px: self.tile_height_px,
            total_height_px: self.total_height_px,
            page_height_pt: self.page_height_pt,
            visual_lines: self.visual_lines.clone(),
            tile_hashes: self.compute_tile_hashes(),
        }
    }

    /// Render both content and sidebar tiles for a given index.
    pub fn render_tile_pair(&self, idx: usize) -> Result<TilePngs> {
        let content = self.render_tile(idx)?;
        let sidebar = self.render_sidebar_tile(idx)?;
        Ok(TilePngs { content, sidebar })
    }
}

// ---------------------------------------------------------------------------
// TiledDocumentCache — external cache for rendered tile PNGs
// ---------------------------------------------------------------------------

/// A pair of rendered PNGs: content + sidebar for the same tile index.
#[derive(Serialize, Deserialize)]
pub struct TilePngs {
    pub content: Vec<u8>,
    pub sidebar: Vec<u8>,
}

/// Cache for rendered tile PNGs, separated from [`TiledDocument`] to allow
/// concurrent `&TiledDocument` access (e.g., from a prefetch worker thread)
/// while the main thread owns `&mut TiledDocumentCache`.
pub struct TiledDocumentCache {
    data: HashMap<usize, TilePngs>,
}

impl Default for TiledDocumentCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TiledDocumentCache {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub fn get(&self, idx: usize) -> Option<&TilePngs> {
        self.data.get(&idx)
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.data.contains_key(&idx)
    }

    pub fn insert(&mut self, idx: usize, pngs: TilePngs) {
        self.data.insert(idx, pngs);
    }

    /// Evict entries far from `center`, keeping only those within `keep_radius`.
    pub fn evict_distant(&mut self, center: usize, keep_radius: usize) {
        let to_evict: Vec<usize> = self
            .data
            .keys()
            .filter(|&&k| (k as isize - center as isize).unsigned_abs() > keep_radius)
            .copied()
            .collect();
        for k in to_evict {
            self.data.remove(&k);
            trace!("cache evict tile {}", k);
        }
    }

    pub fn remove(&mut self, idx: usize) -> Option<TilePngs> {
        self.data.remove(&idx)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}

/// Merge cached tile PNGs from a previous build into a new cache.
///
/// For each tile in the new document, if the old document has a tile with
/// the same content hash, move the PNG over. Returns the new cache.
pub fn merge_tile_cache(
    new_hashes: &[TilePairHash],
    old_hashes: &[TilePairHash],
    old_cache: &mut TiledDocumentCache,
) -> TiledDocumentCache {
    let mut new_cache = TiledDocumentCache::new();
    for (new_idx, new_hash) in new_hashes.iter().enumerate() {
        if let Some(old_idx) = old_hashes.iter().position(|h| h == new_hash)
            && let Some(pngs) = old_cache.remove(old_idx)
        {
            new_cache.insert(new_idx, pngs);
        }
    }
    new_cache
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // TileHash tests
    // -----------------------------------------------------------------------

    /// Create a simple Frame with a rect shape for hash testing.
    fn make_test_frame(width: f64, height: f64) -> Frame {
        let mut frame = Frame::hard(Axes::new(Abs::pt(width), Abs::pt(height)));
        let shape = typst::visualize::Shape {
            geometry: Geometry::Rect(Axes::new(Abs::pt(width), Abs::pt(height))),
            fill: None,
            fill_rule: Default::default(),
            stroke: None,
        };
        frame.push(Point::zero(), FrameItem::Shape(shape, Span::detached()));
        frame
    }

    #[test]
    fn compute_tile_hash_same_frame_same_hash() {
        let f1 = make_test_frame(100.0, 200.0);
        let f2 = make_test_frame(100.0, 200.0);
        assert_eq!(compute_tile_hash(&f1), compute_tile_hash(&f2));
    }

    #[test]
    fn compute_tile_hash_different_frame_different_hash() {
        let f1 = make_test_frame(100.0, 200.0);
        let f2 = make_test_frame(100.0, 201.0);
        assert_ne!(compute_tile_hash(&f1), compute_tile_hash(&f2));
    }

    #[test]
    fn compute_tile_hash_empty_frames() {
        let f1 = Frame::hard(Axes::new(Abs::pt(100.0), Abs::pt(100.0)));
        let f2 = Frame::hard(Axes::new(Abs::pt(100.0), Abs::pt(100.0)));
        assert_eq!(compute_tile_hash(&f1), compute_tile_hash(&f2));
    }

    // -----------------------------------------------------------------------
    // merge_tile_cache tests
    // -----------------------------------------------------------------------

    fn make_hash(v: u8) -> TilePairHash {
        TilePairHash {
            content: TileHash(v as u64),
            sidebar: TileHash(v as u64),
        }
    }

    fn make_pngs(tag: u8) -> TilePngs {
        TilePngs {
            content: vec![tag],
            sidebar: vec![tag],
        }
    }

    #[test]
    fn merge_tile_cache_full_match() {
        let hashes = vec![make_hash(1), make_hash(2)];
        let mut old_cache = TiledDocumentCache::new();
        old_cache.insert(0, make_pngs(10));
        old_cache.insert(1, make_pngs(20));

        let new = merge_tile_cache(&hashes, &hashes, &mut old_cache);
        assert_eq!(new.len(), 2);
        assert_eq!(new.get(0).unwrap().content, vec![10]);
        assert_eq!(new.get(1).unwrap().content, vec![20]);
    }

    #[test]
    fn merge_tile_cache_partial_match() {
        let old_hashes = vec![make_hash(1), make_hash(2)];
        let new_hashes = vec![make_hash(1), make_hash(3)];
        let mut old_cache = TiledDocumentCache::new();
        old_cache.insert(0, make_pngs(10));
        old_cache.insert(1, make_pngs(20));

        let new = merge_tile_cache(&new_hashes, &old_hashes, &mut old_cache);
        assert_eq!(new.len(), 1);
        assert!(new.contains(0));
        assert!(!new.contains(1));
    }

    #[test]
    fn merge_tile_cache_zero_match() {
        let old_hashes = vec![make_hash(1), make_hash(2)];
        let new_hashes = vec![make_hash(3), make_hash(4)];
        let mut old_cache = TiledDocumentCache::new();
        old_cache.insert(0, make_pngs(10));
        old_cache.insert(1, make_pngs(20));

        let new = merge_tile_cache(&new_hashes, &old_hashes, &mut old_cache);
        assert_eq!(new.len(), 0);
    }

    #[test]
    fn merge_tile_cache_evicted_not_recovered() {
        let hashes = vec![make_hash(1)];
        let mut old_cache = TiledDocumentCache::new();
        // Don't insert anything — simulates evicted tile (no PNG in cache)

        let new = merge_tile_cache(&hashes, &hashes, &mut old_cache);
        assert_eq!(new.len(), 0);
    }

    fn make_vl(md_line_range: Option<(usize, usize)>) -> VisualLine {
        VisualLine {
            y_pt: 0.0,
            y_px: 0,
            md_line_range,
            md_line_exact: None,
        }
    }

    #[test]
    fn test_extract_urls_single_link() {
        let md = "Check [Rust](https://rust.invalid/) for details.\n";
        let vls = vec![make_vl(Some((1, 1)))];
        let urls = extract_urls(md, &vls, 0);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://rust.invalid/");
        assert_eq!(urls[0].text, "Rust");
    }

    #[test]
    fn test_extract_urls_multiple_links() {
        let md = "See [A](https://a.invalid/) and [B](https://b.invalid/).\n";
        let vls = vec![make_vl(Some((1, 1)))];
        let urls = extract_urls(md, &vls, 0);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://a.invalid/");
        assert_eq!(urls[0].text, "A");
        assert_eq!(urls[1].url, "https://b.invalid/");
        assert_eq!(urls[1].text, "B");
    }

    #[test]
    fn test_extract_urls_no_links() {
        let md = "Just plain text, no links here.\n";
        let vls = vec![make_vl(Some((1, 1)))];
        let urls = extract_urls(md, &vls, 0);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_urls_no_source_mapping() {
        let md = "Has [link](https://example.invalid/) but no mapping.\n";
        let vls = vec![make_vl(None)];
        let urls = extract_urls(md, &vls, 0);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_urls_out_of_bounds() {
        let md = "Some text\n";
        let vls = vec![make_vl(Some((1, 1)))];
        let urls = extract_urls(md, &vls, 5);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_urls_multiline_block() {
        let md = "Line 1\n[link1](https://one.invalid/)\n[link2](https://two.invalid/)\nLine 4\n";
        let vls = vec![make_vl(Some((2, 3)))];
        let urls = extract_urls(md, &vls, 0);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://one.invalid/");
        assert_eq!(urls[0].text, "link1");
        assert_eq!(urls[1].url, "https://two.invalid/");
        assert_eq!(urls[1].text, "link2");
    }

    #[test]
    fn test_extract_urls_bare_url() {
        let md = "Check https://rust-lang.invalid/ for more\n";
        let urls = extract_urls_from_lines(md, 1, 1);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://rust-lang.invalid/");
        assert_eq!(urls[0].text, "https://rust-lang.invalid/");
    }

    #[test]
    fn test_extract_urls_mixed_link_and_bare() {
        let md = "[Rust](https://rust-lang.invalid) and https://crates.invalid\n";
        let urls = extract_urls_from_lines(md, 1, 1);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://rust-lang.invalid");
        assert_eq!(urls[0].text, "Rust");
        assert_eq!(urls[1].url, "https://crates.invalid");
        assert_eq!(urls[1].text, "https://crates.invalid");
    }

    #[test]
    fn test_extract_urls_bare_duplicate_with_link() {
        let md = "[Rust](https://rust-lang.invalid) and https://rust-lang.invalid\n";
        let urls = extract_urls_from_lines(md, 1, 1);
        assert_eq!(urls.len(), 1, "duplicate bare URL should be deduplicated");
        assert_eq!(urls[0].url, "https://rust-lang.invalid");
        assert_eq!(urls[0].text, "Rust");
    }

    #[test]
    fn test_extract_urls_bare_urls_in_list() {
        let md = "- https://help.x.com/ja/using-x/create-a-thread\n- https://help.x.com/en/using-x/types-of-posts\n";
        let urls = extract_urls_from_lines(md, 1, 2);
        assert_eq!(urls.len(), 2, "each list item should produce one URL");
        assert_eq!(urls[0].url, "https://help.x.com/ja/using-x/create-a-thread");
        assert_eq!(urls[1].url, "https://help.x.com/en/using-x/types-of-posts");
    }
}
