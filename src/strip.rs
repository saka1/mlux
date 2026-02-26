use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

use anyhow::Result;
use log::{debug, info, trace};
use typst::foundations::Smart;
use typst::layout::{Abs, Axes, Frame, FrameItem, Page, PagedDocument, Point};
use typst::syntax::{Source, Span};
use typst::visualize::{Geometry, Paint};

use crate::convert::SourceMap;

/// A visual line extracted from the PagedDocument frame tree.
#[derive(Debug, Clone)]
pub struct VisualLine {
    pub y_pt: f64, // Absolute Y coordinate of the text baseline (pt)
    pub y_px: u32, // Pixel Y coordinate (after ppi conversion)
    /// Markdown source line range (1-based, inclusive). None for theme-derived text.
    pub md_line_range: Option<(usize, usize)>,
    /// Precise 1-based MD line for code blocks (y yank). None for non-code blocks.
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

    // Collect (Y coordinate, representative Span) from all TextItem nodes.
    let mut entries: Vec<(f64, Option<Span>)> = Vec::new();
    collect_text_y_span(&document.pages[0].frame, Point::zero(), &mut entries);

    // Sort and deduplicate by Y coordinate.
    //
    // Tolerance is 5pt: within a single visual line, different font sizes
    // (e.g., 12pt body vs 10pt inline code) produce baseline offsets of
    // up to ~2.6pt (0.59pt font metric diff + 2pt inset). The minimum
    // inter-line gap is ~15pt (heading → body), so 5pt safely merges
    // intra-line variants without collapsing separate lines.
    const TOLERANCE_PT: f64 = 5.0;

    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut deduped: Vec<(f64, Option<Span>)> = Vec::new();
    for (y, span) in entries {
        if deduped
            .last()
            .map_or(true, |(prev_y, _)| (y - prev_y).abs() > TOLERANCE_PT)
        {
            deduped.push((y, span));
        } else if let Some(last) = deduped.last_mut() {
            // Within tolerance — prefer a non-detached span over a detached one
            if let Some(s) = span {
                if !s.is_detached() && last.1.map_or(true, |ls| ls.is_detached()) {
                    last.1 = Some(s);
                }
            }
        }
    }

    let pixel_per_pt = ppi as f64 / 72.0;
    let lines: Vec<VisualLine> = deduped
        .into_iter()
        .enumerate()
        .map(|(i, (y_pt, span))| {
            trace!("visual_line[{i}]: y={y_pt:.1}pt, span={}", span.map_or("none".to_string(), |s| if s.is_detached() { "detached".to_string() } else { "attached".to_string() }));
            let info = mapping
                .and_then(|m| resolve_md_line_range(span?, m));
            VisualLine {
                y_pt,
                y_px: (y_pt * pixel_per_pt).round() as u32,
                md_line_range: info.as_ref().map(|i| i.range),
                md_line_exact: info.as_ref().and_then(|i| i.exact),
            }
        })
        .collect();

    info!(
        "strip: extract_visual_lines completed in {:.1}ms ({} lines)",
        start.elapsed().as_secs_f64() * 1000.0,
        lines.len()
    );
    if mapping.is_some() {
        let mapped = lines.iter().filter(|l| l.md_line_range.is_some()).count();
        debug!(
            "extract_visual_lines: {} lines ({} mapped, {} unmapped)",
            lines.len(),
            mapped,
            lines.len() - mapped
        );
        for (i, vl) in lines.iter().enumerate() {
            if let Some((s, e)) = vl.md_line_range {
                let preview: String = mapping.unwrap().md_source
                    .lines()
                    .nth(s - 1)
                    .unwrap_or("")
                    .chars()
                    .take(60)
                    .collect();
                debug!(
                    "  vl[{i}] y={:.1}pt → md L{s}-{e}: {:?}",
                    vl.y_pt, preview
                );
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
/// For code blocks, also computes `exact`: the precise 1-based MD line
/// corresponding to this span position within the block.
fn resolve_md_line_range(span: Span, params: &SourceMappingParams) -> Option<MdLineInfo> {
    if span.is_detached() {
        trace!("  span detached, skipping");
        return None;
    }

    // Resolve Span to byte range in main.typ
    let main_range = params.source.range(span)?;

    // Convert to content_text offset
    if main_range.start < params.content_offset {
        trace!("  span in prefix (main_range={:?}, content_offset={})", main_range, params.content_offset);
        return None; // Within theme/prefix, not content
    }
    let content_offset = main_range.start - params.content_offset;

    // Look up in SourceMap
    let block = params.source_map.find_by_typst_offset(content_offset)?;

    // Convert md_byte_range to line numbers (1-based)
    let start_line = byte_offset_to_line(params.md_source, block.md_byte_range.start);
    let end_line = byte_offset_to_line(
        params.md_source,
        block.md_byte_range.end.saturating_sub(1).max(block.md_byte_range.start),
    );

    // Compute exact line for code blocks.
    // Code blocks in Markdown start with "```"; their Typst output preserves
    // the same line structure (fill_blank_lines inserts spaces but keeps newline count).
    // We count newlines in the Typst text before the span position to find the
    // exact source line within the block.
    let md_block_text = &params.md_source[block.md_byte_range.clone()];
    let exact = if md_block_text.starts_with("```") {
        let typst_local_offset = content_offset - block.typst_byte_range.start;
        let typst_block_text = params.source.text().get(
            (block.typst_byte_range.start + params.content_offset)
                ..(block.typst_byte_range.end + params.content_offset),
        );
        if let Some(typst_text) = typst_block_text {
            let clamped = typst_local_offset.min(typst_text.len());
            let newlines_before = typst_text[..clamped]
                .bytes()
                .filter(|&b| b == b'\n')
                .count();
            // start_line is the "```" fence line; content starts at start_line + 1
            let exact_line = start_line + 1 + newlines_before;
            // Clamp to not exceed end_line - 1 (closing fence)
            let exact_line = exact_line.min(end_line.saturating_sub(1)).max(start_line + 1);
            trace!("  code block exact: typst_local_off={}, newlines={}, exact_line={}", typst_local_offset, newlines_before, exact_line);
            Some(exact_line)
        } else {
            None
        }
    } else {
        None
    };

    trace!(
        "  span resolved: main={:?} → content_off={} → typst_block={:?} → md_block={:?} → lines {}-{} exact={:?}",
        main_range, content_offset, block.typst_byte_range, block.md_byte_range, start_line, end_line, exact
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

/// Recursively collect (absolute Y, representative Span) from all TextItem nodes.
fn collect_text_y_span(
    frame: &Frame,
    parent_offset: Point,
    out: &mut Vec<(f64, Option<Span>)>,
) {
    for (pos, item) in frame.items() {
        let abs = parent_offset + *pos;
        match item {
            FrameItem::Text(text) => {
                let span = text.glyphs.first().map(|g| g.span.0);
                out.push((abs.y.to_pt(), span));
            }
            FrameItem::Group(group) => {
                collect_text_y_span(&group.frame, abs, out);
            }
            _ => {}
        }
    }
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
    let end_idx = max_line.min(lines.len());     // 1-based end → exclusive 0-based

    if start_idx >= lines.len() {
        return String::new();
    }

    lines[start_idx..end_idx].join("\n")
}

/// Extract the precise Markdown source line for a visual line.
///
/// For code blocks, returns the single line indicated by `md_line_exact`.
/// For other blocks (paragraphs, headings, etc.), falls back to block-level
/// yank via `yank_lines`.
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

/// Generate Typst source for the sidebar image.
///
/// Uses `#place()` to position line numbers at the exact Y coordinates
/// extracted from the content document's frame tree.
pub fn generate_sidebar_typst(
    lines: &[VisualLine],
    sidebar_width_pt: f64,
    page_height_pt: f64,
) -> String {
    let mut src = String::new();
    writeln!(
        src,
        "#set page(width: {sidebar_width_pt:.1}pt, height: {page_height_pt:.1}pt, margin: 0pt, fill: rgb(\"#1e1e2e\"))"
    )
    .unwrap();
    writeln!(
        src,
        "#set text(font: \"DejaVu Sans Mono\", size: 8pt, fill: rgb(\"#6c7086\"))"
    )
    .unwrap();

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        let dy = line.y_pt;
        // Place at baseline Y; use top+right alignment with ascent offset.
        // 8pt text has ~6pt ascent, so shift up to align baselines.
        writeln!(
            src,
            "#place(top + right, dy: {dy:.1}pt - 6pt, dx: -4pt)[#text(size: 8pt)[{line_num}]]"
        )
        .unwrap();
    }

    src
}

// ---------------------------------------------------------------------------
// Frame splitting — split a tall frame into vertical strips
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
                            let ys =
                                [p1.y.to_pt(), p2.y.to_pt(), p3.y.to_pt()];
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

/// Split a compiled Frame into vertical strips of `strip_height_pt` each.
///
/// Items spanning a strip boundary are cloned into both strips.
/// tiny-skia clips drawing outside the canvas, so the visual result is correct.
pub fn split_frame(frame: &Frame, strip_height_pt: f64) -> Vec<Frame> {
    let start = Instant::now();
    let total_height = frame.size().y.to_pt();
    let strip_count = (total_height / strip_height_pt).ceil().max(1.0) as usize;
    let orig_width = frame.size().x;

    let mut strips = Vec::with_capacity(strip_count);

    for i in 0..strip_count {
        let y_start = i as f64 * strip_height_pt;
        let y_end = ((i + 1) as f64 * strip_height_pt).min(total_height);
        let strip_h = y_end - y_start;

        let mut sub = Frame::hard(Axes {
            x: orig_width,
            y: Abs::pt(strip_h),
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

                // Check if item spans beyond this strip
                if item_y < y_start || item_bottom > y_end {
                    spanning_count += 1;
                }
            }
        }

        debug!(
            "strip {}: {} items, {} boundary-spanning",
            i, item_count, spanning_count
        );
        strips.push(sub);
    }

    info!(
        "strip: split_frame completed in {:.1}ms ({} strips, height={}pt)",
        start.elapsed().as_secs_f64() * 1000.0,
        strip_count,
        strip_height_pt
    );
    strips
}

// ---------------------------------------------------------------------------
// StripDocument — lazy strip-based rendering
// ---------------------------------------------------------------------------

/// Which strips are visible for a given scroll position.
#[derive(Debug)]
pub enum VisibleStrips {
    /// Viewport fits entirely within one strip.
    Single {
        idx: usize,
        src_y: u32,
        src_h: u32,
    },
    /// Viewport straddles two strips.
    Split {
        top_idx: usize,
        top_src_y: u32,
        top_src_h: u32,
        bot_idx: usize,
        bot_src_h: u32,
    },
}

/// A document split into renderable strips for lazy, bounded-memory rendering.
///
/// All methods take `&self` — rendering is pure (no internal caching).
/// Use [`StripDocumentCache`] separately for caching rendered PNGs.
pub struct StripDocument {
    strips: Vec<Frame>,
    sidebar_strips: Vec<Frame>,
    sidebar_fill: Smart<Option<Paint>>,
    page_fill: Smart<Option<Paint>>,
    ppi: f32,
    width_px: u32,
    sidebar_width_px: u32,
    strip_height_px: u32,
    total_height_px: u32,
    page_height_pt: f64,
    pub visual_lines: Vec<VisualLine>,
}

impl StripDocument {
    /// Build a StripDocument from a compiled content + sidebar PagedDocument.
    ///
    /// - `sidebar_doc`: compiled sidebar document (same page height as content)
    /// - `visual_lines`: pre-extracted visual line positions (avoids redundant extraction)
    /// - `strip_height_pt`: height of each strip in typst points
    /// - `ppi`: rendering resolution
    pub fn new(
        document: &PagedDocument,
        sidebar_doc: &PagedDocument,
        visual_lines: Vec<VisualLine>,
        strip_height_pt: f64,
        ppi: f32,
    ) -> Self {
        assert!(!document.pages.is_empty(), "document has no pages");
        let page = &document.pages[0];

        let page_size = page.frame.size();
        info!(
            "compiled: {:.1}x{:.1}pt, {} top-level items",
            page_size.x.to_pt(),
            page_size.y.to_pt(),
            page.frame.items().count()
        );

        let strips = split_frame(&page.frame, strip_height_pt);

        // Split sidebar with the same strip boundaries
        assert!(!sidebar_doc.pages.is_empty(), "sidebar document has no pages");
        let sidebar_page = &sidebar_doc.pages[0];
        let sidebar_strips = split_frame(&sidebar_page.frame, strip_height_pt);
        let pixel_per_pt = ppi as f64 / 72.0;
        let sidebar_width_px = (sidebar_page.frame.size().x.to_pt() * pixel_per_pt).ceil() as u32;
        info!(
            "sidebar: {} strips, {}px wide",
            sidebar_strips.len(),
            sidebar_width_px
        );

        let width_px = (page_size.x.to_pt() * pixel_per_pt).ceil() as u32;
        let strip_height_px = (strip_height_pt * pixel_per_pt).ceil() as u32;
        let total_height_px = (page_size.y.to_pt() * pixel_per_pt).ceil() as u32;
        let page_height_pt = page_size.y.to_pt();

        Self {
            strips,
            sidebar_strips,
            sidebar_fill: sidebar_page.fill.clone(),
            page_fill: page.fill.clone(),
            ppi,
            width_px,
            sidebar_width_px,
            strip_height_px,
            total_height_px,
            page_height_pt,
            visual_lines,
        }
    }

    /// Number of strips.
    pub fn strip_count(&self) -> usize {
        self.strips.len()
    }

    /// Document width in pixels.
    pub fn width_px(&self) -> u32 {
        self.width_px
    }

    /// Sidebar width in pixels.
    pub fn sidebar_width_px(&self) -> u32 {
        self.sidebar_width_px
    }

    /// Height of one standard strip in pixels.
    pub fn strip_height_px(&self) -> u32 {
        self.strip_height_px
    }

    /// Total document height in pixels.
    pub fn total_height_px(&self) -> u32 {
        self.total_height_px
    }

    /// Page height in typst points (for sidebar generation).
    pub fn page_height_pt(&self) -> f64 {
        self.page_height_pt
    }

    /// Actual pixel height of a specific strip (last strip may be shorter).
    fn strip_actual_height_px(&self, idx: usize) -> u32 {
        let pixel_per_pt = self.ppi as f64 / 72.0;
        (self.strips[idx].size().y.to_pt() * pixel_per_pt).ceil() as u32
    }

    /// Render a single content strip to PNG bytes.
    ///
    /// This is a pure function -- no internal caching.
    /// Thread-safe: `Frame.items` uses `Arc`, `typst_render::render` is stateless.
    pub fn render_strip(&self, idx: usize) -> Result<Vec<u8>> {
        self.render_frame(idx, &self.strips, &self.page_fill, "content")
    }

    /// Render a single sidebar strip to PNG bytes.
    pub fn render_sidebar_strip(&self, idx: usize) -> Result<Vec<u8>> {
        self.render_frame(idx, &self.sidebar_strips, &self.sidebar_fill, "sidebar")
    }

    /// Render a frame from `strips` at `idx` to PNG bytes.
    fn render_frame(
        &self,
        idx: usize,
        strips: &[Frame],
        fill: &Smart<Option<Paint>>,
        label: &str,
    ) -> Result<Vec<u8>> {
        assert!(idx < strips.len(), "{label} strip index out of bounds");
        trace!("rendering {label} strip {idx}");
        let start = Instant::now();

        let page = Page {
            frame: strips[idx].clone(),
            fill: fill.clone(),
            numbering: None,
            supplement: typst::foundations::Content::empty(),
            number: 0,
        };

        let pixel_per_pt = self.ppi / 72.0;
        let pixmap = typst_render::render(&page, pixel_per_pt);
        let png = pixmap
            .encode_png()
            .map_err(|e| anyhow::anyhow!("{label} PNG encoding failed: {e}"))?;

        debug!(
            "render {label} strip {idx}: {}x{}px, {} bytes PNG in {:.1}ms",
            pixmap.width(),
            pixmap.height(),
            png.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );

        Ok(png)
    }

    /// Determine which strip(s) are visible at a given scroll offset.
    pub fn visible_strips(&self, global_y: u32, vp_h: u32) -> VisibleStrips {
        let top_strip = (global_y / self.strip_height_px) as usize;
        let top_strip = top_strip.min(self.strips.len().saturating_sub(1));
        let src_y_in_strip = global_y - (top_strip as u32 * self.strip_height_px);
        let top_actual_h = self.strip_actual_height_px(top_strip);
        let remaining_in_top = top_actual_h.saturating_sub(src_y_in_strip);

        if remaining_in_top >= vp_h || top_strip + 1 >= self.strips.len() {
            // Viewport fits in one strip (or no more strips)
            let src_h = vp_h.min(remaining_in_top);
            debug!(
                "display: single strip {}, src_y={}, src_h={}, vp_h={}",
                top_strip, src_y_in_strip, src_h, vp_h
            );
            VisibleStrips::Single {
                idx: top_strip,
                src_y: src_y_in_strip,
                src_h,
            }
        } else {
            // Viewport straddles two strips
            let top_src_h = remaining_in_top;
            let bot_idx = top_strip + 1;
            let bot_src_h = (vp_h - top_src_h).min(self.strip_actual_height_px(bot_idx));
            debug!(
                "display: split strips [{}, {}], top_src_y={}, top_h={}, bot_h={}, vp_h={}",
                top_strip, bot_idx, src_y_in_strip, top_src_h, bot_src_h, vp_h
            );
            VisibleStrips::Split {
                top_idx: top_strip,
                top_src_y: src_y_in_strip,
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
}

// ---------------------------------------------------------------------------
// StripDocumentCache — external cache for rendered strip PNGs
// ---------------------------------------------------------------------------

/// A pair of rendered PNGs: content + sidebar for the same strip index.
pub struct StripPngs {
    pub content: Vec<u8>,
    pub sidebar: Vec<u8>,
}

/// Cache for rendered strip PNGs, separated from [`StripDocument`] to allow
/// concurrent `&StripDocument` access (e.g., from a prefetch worker thread)
/// while the main thread owns `&mut StripDocumentCache`.
pub struct StripDocumentCache {
    data: HashMap<usize, StripPngs>,
}

impl StripDocumentCache {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub fn get(&self, idx: usize) -> Option<&StripPngs> {
        self.data.get(&idx)
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.data.contains_key(&idx)
    }

    pub fn insert(&mut self, idx: usize, pngs: StripPngs) {
        self.data.insert(idx, pngs);
    }

    /// Get cached PNGs or render synchronously and cache the result.
    pub fn get_or_render(&mut self, doc: &StripDocument, idx: usize) -> Result<&StripPngs> {
        if !self.data.contains_key(&idx) {
            let content = doc.render_strip(idx)?;
            let sidebar = doc.render_sidebar_strip(idx)?;
            self.data.insert(idx, StripPngs { content, sidebar });
        }
        Ok(self.data.get(&idx).unwrap())
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
            trace!("cache evict strip {}", k);
        }
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}
