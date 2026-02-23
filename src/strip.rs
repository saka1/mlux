use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;

use anyhow::Result;
use log::{debug, info, trace};
use typst::foundations::Smart;
use typst::layout::{Abs, Axes, Frame, FrameItem, Page, PagedDocument, Point};
use typst::visualize::{Geometry, Paint};

/// A visual line extracted from the PagedDocument frame tree.
#[derive(Debug, Clone)]
pub struct VisualLine {
    pub y_pt: f64, // Absolute Y coordinate of the text baseline (pt)
    pub y_px: u32, // Pixel Y coordinate (after ppi conversion)
}

/// Extract visual line positions from the frame tree.
///
/// Walks all TextItem nodes, collects their absolute Y coordinates,
/// deduplicates with some tolerance, and returns sorted VisualLines.
pub fn extract_visual_lines(document: &PagedDocument, ppi: f32) -> Vec<VisualLine> {
    if document.pages.is_empty() {
        return Vec::new();
    }

    let mut y_coords: Vec<f64> = Vec::new();
    collect_text_y(&document.pages[0].frame, Point::zero(), &mut y_coords);

    // Sort and deduplicate.
    //
    // Tolerance is 5pt: within a single visual line, different font sizes
    // (e.g., 12pt body vs 10pt inline code) produce baseline offsets of
    // up to ~2.6pt (0.59pt font metric diff + 2pt inset). The minimum
    // inter-line gap is ~15pt (heading → body), so 5pt safely merges
    // intra-line variants without collapsing separate lines.
    const TOLERANCE_PT: f64 = 5.0;

    y_coords.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut deduped: Vec<f64> = Vec::new();
    for y in y_coords {
        if deduped.last().map_or(true, |prev| (y - prev).abs() > TOLERANCE_PT) {
            deduped.push(y);
        }
    }

    let pixel_per_pt = ppi as f64 / 72.0;
    deduped
        .into_iter()
        .map(|y_pt| VisualLine {
            y_pt,
            y_px: (y_pt * pixel_per_pt).round() as u32,
        })
        .collect()
}

/// Recursively collect absolute Y coordinates of all TextItem nodes.
fn collect_text_y(frame: &Frame, parent_offset: Point, out: &mut Vec<f64>) {
    for (pos, item) in frame.items() {
        let abs = parent_offset + *pos;
        match item {
            FrameItem::Text(_) => {
                out.push(abs.y.to_pt());
            }
            FrameItem::Group(group) => {
                collect_text_y(&group.frame, abs, out);
            }
            _ => {}
        }
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
        "split into {} strips (height={}pt)",
        strip_count, strip_height_pt
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
pub struct StripDocument {
    strips: Vec<Frame>,
    page_fill: Smart<Option<Paint>>,
    ppi: f32,
    width_px: u32,
    strip_height_px: u32,
    total_height_px: u32,
    page_height_pt: f64,
    pub visual_lines: Vec<VisualLine>,
    cache: HashMap<usize, Vec<u8>>,
    lru: VecDeque<usize>,
    cache_size: usize,
}

impl StripDocument {
    /// Build a StripDocument from a compiled PagedDocument.
    ///
    /// - `strip_height_pt`: height of each strip in typst points
    /// - `ppi`: rendering resolution
    /// - `cache_size`: max cached strip PNGs (0 = no caching)
    pub fn new(
        document: &PagedDocument,
        strip_height_pt: f64,
        ppi: f32,
        cache_size: usize,
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

        let visual_lines = extract_visual_lines(document, ppi);
        info!("extracted {} visual lines", visual_lines.len());

        let strips = split_frame(&page.frame, strip_height_pt);

        let pixel_per_pt = ppi as f64 / 72.0;
        let width_px = (page_size.x.to_pt() * pixel_per_pt).ceil() as u32;
        let strip_height_px = (strip_height_pt * pixel_per_pt).ceil() as u32;
        let total_height_px = (page_size.y.to_pt() * pixel_per_pt).ceil() as u32;
        let page_height_pt = page_size.y.to_pt();

        Self {
            strips,
            page_fill: page.fill.clone(),
            ppi,
            width_px,
            strip_height_px,
            total_height_px,
            page_height_pt,
            visual_lines,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            cache_size,
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

    /// Render a single strip to PNG bytes (with optional LRU caching).
    pub fn get_strip_png(&mut self, idx: usize) -> Result<&[u8]> {
        assert!(idx < self.strips.len(), "strip index out of bounds");

        if self.cache_size > 0 && self.cache.contains_key(&idx) {
            // Move to front of LRU
            self.lru.retain(|&i| i != idx);
            self.lru.push_back(idx);
            trace!("cache hit strip {}", idx);
            return Ok(self.cache.get(&idx).unwrap());
        }

        trace!("cache miss strip {}, rendering", idx);

        // Build a Page from the sub-frame
        let page = Page {
            frame: self.strips[idx].clone(),
            fill: self.page_fill.clone(),
            numbering: None,
            supplement: typst::foundations::Content::empty(),
            number: 0,
        };

        let pixel_per_pt = self.ppi / 72.0;
        let pixmap = typst_render::render(&page, pixel_per_pt);
        let png = pixmap
            .encode_png()
            .map_err(|e| anyhow::anyhow!("PNG encoding failed: {e}"))?;

        debug!(
            "render strip {}: {}x{}px, {} bytes PNG",
            idx,
            pixmap.width(),
            pixmap.height(),
            png.len()
        );

        if self.cache_size > 0 {
            // Evict if at capacity
            while self.lru.len() >= self.cache_size {
                if let Some(evicted) = self.lru.pop_front() {
                    self.cache.remove(&evicted);
                    trace!("cache evict strip {}", evicted);
                }
            }
            self.cache.insert(idx, png);
            self.lru.push_back(idx);
            Ok(self.cache.get(&idx).unwrap())
        } else {
            // No caching: store in slot 0, evicting previous
            self.cache.clear();
            self.cache.insert(idx, png);
            Ok(self.cache.get(&idx).unwrap())
        }
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
