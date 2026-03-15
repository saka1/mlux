//! Span-based search highlighting via KGP overlay.
//!
//! The caller provides pre-computed `target_ranges` (main.typ byte ranges)
//! from `ContentIndex::md_to_main_ranges()`. This module walks the Frame tree,
//! resolves each glyph's position in main.typ via `Source::range()` +
//! `rendered_to_source_byte()`, and generates pixel rectangles for matching glyphs.

use std::ops::Range;
use std::time::Instant;

use log::debug;
use serde::{Deserialize, Serialize};
use typst::layout::{Frame, FrameItem, Point};
use typst::syntax::Source;
use typst::text::TextItem;

use crate::pipeline::rendered_to_source_byte;

/// Specification for what to highlight, sent via IPC to the fork child.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HighlightSpec {
    /// Pre-computed byte ranges within main.typ to highlight (sorted).
    pub target_ranges: Vec<Range<usize>>,
    /// Byte ranges for the currently active match (subset of target_ranges).
    /// Glyphs in these ranges get `is_active = true` on their `HighlightRect`.
    pub active_ranges: Vec<Range<usize>>,
}

/// A pixel-coordinate rectangle to draw as a highlight overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightRect {
    pub x_px: u32,
    pub y_px: u32,
    pub w_px: u32,
    pub h_px: u32,
    /// Whether this rect belongs to the currently active/selected match.
    pub is_active: bool,
}

/// Find all highlight rectangles for glyphs whose source positions fall within
/// `spec.target_ranges`, within a single tile frame.
pub fn find_highlight_rects(
    frame: &Frame,
    spec: &HighlightSpec,
    ppi: f32,
    source: &Source,
) -> Vec<HighlightRect> {
    if spec.target_ranges.is_empty() {
        return Vec::new();
    }

    let start = Instant::now();

    let pixel_per_pt = ppi / 72.0;
    let mut rects = Vec::new();
    walk_frame_by_span(
        frame,
        Point::zero(),
        &spec.target_ranges,
        &spec.active_ranges,
        pixel_per_pt,
        source,
        &mut rects,
    );

    debug!(
        "highlight: find_highlight_rects completed in {:.1}ms ({} rects, {} target_ranges)",
        start.elapsed().as_secs_f64() * 1000.0,
        rects.len(),
        spec.target_ranges.len(),
    );

    rects
}

/// Recursively walk the frame tree, resolving glyph spans to main.typ positions.
fn walk_frame_by_span(
    frame: &Frame,
    offset: Point,
    target_ranges: &[Range<usize>],
    active_ranges: &[Range<usize>],
    pixel_per_pt: f32,
    source: &Source,
    rects: &mut Vec<HighlightRect>,
) {
    for (pos, item) in frame.items() {
        let abs = Point::new(offset.x + pos.x, offset.y + pos.y);
        match item {
            FrameItem::Text(text) => {
                collect_span_rects(
                    text,
                    abs,
                    target_ranges,
                    active_ranges,
                    pixel_per_pt,
                    source,
                    rects,
                );
            }
            FrameItem::Group(group) => {
                walk_frame_by_span(
                    &group.frame,
                    abs,
                    target_ranges,
                    active_ranges,
                    pixel_per_pt,
                    source,
                    rects,
                );
            }
            _ => {}
        }
    }
}

/// For a single TextItem, check each glyph against target_ranges and collect
/// matching glyph runs as pixel rectangles.
fn collect_span_rects(
    text: &TextItem,
    abs_pos: Point,
    target_ranges: &[Range<usize>],
    active_ranges: &[Range<usize>],
    pixel_per_pt: f32,
    source: &Source,
    rects: &mut Vec<HighlightRect>,
) {
    let glyphs = &text.glyphs;
    if glyphs.is_empty() {
        return;
    }

    // Precompute cumulative x positions for each glyph (in pt).
    let mut glyph_x_starts: Vec<f64> = Vec::with_capacity(glyphs.len());
    let mut cursor = 0.0_f64;
    for g in glyphs.iter() {
        glyph_x_starts.push(cursor + g.x_offset.at(text.size).to_pt());
        cursor += g.x_advance.at(text.size).to_pt();
    }

    let text_height_pt = text.size.to_pt();

    // Glyph run accumulator: collects consecutive matching glyphs into a
    // single highlight rectangle.
    let mut run = GlyphRun::new();

    // Cache the last resolved (Span → node_range) to avoid redundant
    // Source tree walks for consecutive glyphs sharing the same Span.
    let mut cached_span: Option<(typst::syntax::Span, Option<Range<usize>>)> = None;

    for (i, g) in glyphs.iter().enumerate() {
        // Resolve this glyph's absolute position in main.typ, with caching
        let node_range = if cached_span.as_ref().is_some_and(|(s, _)| *s == g.span.0) {
            cached_span.as_ref().unwrap().1.clone()
        } else {
            let nr = source.range(g.span.0);
            cached_span = Some((g.span.0, nr.clone()));
            nr
        };

        let main_pos = node_range.map(|nr| {
            let node_text = &source.text()[nr.clone()];
            let source_byte = rendered_to_source_byte(node_text, g.span.1 as usize);
            nr.start + source_byte
        });

        let is_match = main_pos.is_some_and(|pos| {
            let idx = target_ranges.partition_point(|r| r.end <= pos);
            idx < target_ranges.len() && target_ranges[idx].start <= pos
        });

        if is_match {
            let is_active = !active_ranges.is_empty()
                && main_pos.is_some_and(|pos| {
                    let idx = active_ranges.partition_point(|r| r.end <= pos);
                    idx < active_ranges.len() && active_ranges[idx].start <= pos
                });

            // If active status changes mid-run, flush to split into separate rects.
            if run.start_x.is_some() && run.is_active != is_active {
                run.flush(abs_pos, text_height_pt, pixel_per_pt, rects);
            }

            let x = glyph_x_starts[i];
            let w = g.x_advance.at(text.size).to_pt();
            run.extend(x, x + w, is_active);
        } else {
            run.flush(abs_pos, text_height_pt, pixel_per_pt, rects);
        }
    }
    run.flush(abs_pos, text_height_pt, pixel_per_pt, rects);
}

/// Accumulator for consecutive matching glyphs within a TextItem.
///
/// Tracks the x-extent of the current run and flushes it as a
/// `HighlightRect` when a non-matching glyph is encountered.
struct GlyphRun {
    start_x: Option<f64>,
    end_x: f64,
    is_active: bool,
}

impl GlyphRun {
    fn new() -> Self {
        Self {
            start_x: None,
            end_x: 0.0,
            is_active: false,
        }
    }

    fn extend(&mut self, x: f64, x_end: f64, is_active: bool) {
        if self.start_x.is_none() {
            self.start_x = Some(x);
            self.is_active = is_active;
        }
        self.end_x = x_end;
    }

    fn flush(
        &mut self,
        abs_pos: Point,
        text_height_pt: f64,
        pixel_per_pt: f32,
        rects: &mut Vec<HighlightRect>,
    ) {
        if let Some(start_x) = self.start_x.take() {
            let abs_x_pt = abs_pos.x.to_pt() + start_x;
            let baseline_y_pt = abs_pos.y.to_pt();
            let abs_y_pt = baseline_y_pt - text_height_pt * 0.8;
            let width_pt = self.end_x - start_x;

            if abs_y_pt < 0.0 {
                return;
            }

            let x_px = (abs_x_pt * pixel_per_pt as f64).round() as u32;
            let y_px = (abs_y_pt * pixel_per_pt as f64).round() as u32;
            let w_px = (width_pt * pixel_per_pt as f64).round().max(1.0) as u32;
            let h_px = (text_height_pt * pixel_per_pt as f64).round().max(1.0) as u32;

            rects.push(HighlightRect {
                x_px,
                y_px,
                w_px,
                h_px,
                is_active: self.is_active,
            });
        }
    }
}

/// 2048×24 semi-transparent yellow PNG (RGBA 255, 220, 0, 80).
///
/// Dimensions are chosen to match 12pt text at 144 PPI (= 24 px height).
/// Width is large enough for any highlight span. KGP source-rectangle `w`
/// crops to the exact highlight width; the native height already matches the
/// text line height, so KGP placement with `r=1` requires minimal scaling.
pub const HIGHLIGHT_PNG: &[u8] = include_bytes!("../assets/highlight.png");

/// 2048×24 semi-transparent orange PNG (RGBA 255, 140, 0, 120) for the active match.
pub const HIGHLIGHT_ACTIVE_PNG: &[u8] = include_bytes!("../assets/highlight_active.png");

/// Native width of [`HIGHLIGHT_PNG`] in pixels.
pub const HIGHLIGHT_PNG_WIDTH: u32 = 2048;

/// Native height of [`HIGHLIGHT_PNG`] in pixels.
pub const HIGHLIGHT_PNG_HEIGHT: u32 = 24;

// ---------------------------------------------------------------------------
// Partial-transparency 1×24 RGBA patterns for overflow coverage
// ---------------------------------------------------------------------------

/// Height of the partial-transparency patterns in pixels.
pub const PATTERN_HEIGHT: u32 = 24;

/// Width of the partial-transparency patterns in pixels.
pub const PATTERN_WIDTH: u32 = 1;

/// Generate a 1×24 raw RGBA pattern: top `filled_rows` pixels are
/// semi-transparent yellow (255, 220, 0, 80), remaining are fully transparent.
const fn make_pattern(filled_rows: usize) -> [u8; 96] {
    let mut buf = [0u8; 96];
    let mut i = 0;
    while i < 24 {
        let off = i * 4;
        if i < filled_rows {
            buf[off] = 255; // R
            buf[off + 1] = 220; // G
            buf[off + 2] = 0; // B
            buf[off + 3] = 80; // A
        }
        // else: already zeroed (fully transparent)
        i += 1;
    }
    buf
}

/// Top 25% yellow (6 of 24 rows filled).
pub const PATTERN_P25: [u8; 96] = make_pattern(6);
/// Top 50% yellow (12 of 24 rows filled).
pub const PATTERN_P50: [u8; 96] = make_pattern(12);
/// Top 75% yellow (18 of 24 rows filled).
pub const PATTERN_P75: [u8; 96] = make_pattern(18);

/// Generate a 1×24 raw RGBA pattern with active (orange) color:
/// top `filled_rows` pixels are (255, 140, 0, 120), remaining transparent.
const fn make_pattern_active(filled_rows: usize) -> [u8; 96] {
    let mut buf = [0u8; 96];
    let mut i = 0;
    while i < 24 {
        let off = i * 4;
        if i < filled_rows {
            buf[off] = 255; // R
            buf[off + 1] = 140; // G
            buf[off + 2] = 0; // B
            buf[off + 3] = 120; // A
        }
        i += 1;
    }
    buf
}

/// Top 25% orange (6 of 24 rows filled) — active match.
pub const PATTERN_ACTIVE_P25: [u8; 96] = make_pattern_active(6);
/// Top 50% orange (12 of 24 rows filled) — active match.
pub const PATTERN_ACTIVE_P50: [u8; 96] = make_pattern_active(12);
/// Top 75% orange (18 of 24 rows filled) — active match.
pub const PATTERN_ACTIVE_P75: [u8; 96] = make_pattern_active(18);

/// Which partial pattern to use for overflow placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialPattern {
    P25,
    P50,
    P75,
    Full,
}

/// Select the appropriate overflow pattern based on how many pixels
/// of a highlight rect overflow into the next cell row.
pub fn select_overflow_pattern(overflow_px: u32, ch: u32) -> PartialPattern {
    if ch == 0 {
        return PartialPattern::Full;
    }
    let frac = overflow_px as f32 / ch as f32;
    match () {
        _ if frac <= 0.25 => PartialPattern::P25,
        _ if frac <= 0.50 => PartialPattern::P50,
        _ if frac <= 0.75 => PartialPattern::P75,
        _ => PartialPattern::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_target_ranges_returns_no_rects() {
        let frame = Frame::hard(typst::layout::Size::zero());
        let spec = HighlightSpec {
            target_ranges: vec![],
            active_ranges: vec![],
        };
        let source = Source::detached("");
        let rects = find_highlight_rects(&frame, &spec, 144.0, &source);
        assert!(rects.is_empty());
    }

    #[test]
    fn highlight_png_is_valid() {
        assert!(HIGHLIGHT_PNG.len() > 8);
        assert_eq!(&HIGHLIGHT_PNG[1..4], b"PNG");
        assert!(HIGHLIGHT_PNG.len() > 100);
    }

    #[test]
    fn highlight_active_png_is_valid() {
        assert!(HIGHLIGHT_ACTIVE_PNG.len() > 8);
        assert_eq!(&HIGHLIGHT_ACTIVE_PNG[1..4], b"PNG");
        assert!(HIGHLIGHT_ACTIVE_PNG.len() > 100);
    }

    #[test]
    fn active_pattern_p25_has_6_filled_rows() {
        for row in 0..6 {
            let off = row * 4;
            assert_eq!(
                PATTERN_ACTIVE_P25[off + 3],
                120,
                "row {row} should be opaque"
            );
        }
        for row in 6..24 {
            let off = row * 4;
            assert_eq!(
                PATTERN_ACTIVE_P25[off + 3],
                0,
                "row {row} should be transparent"
            );
        }
    }

    #[test]
    fn pattern_p25_has_6_filled_rows() {
        for row in 0..6 {
            let off = row * 4;
            assert_eq!(PATTERN_P25[off + 3], 80, "row {row} should be opaque");
        }
        for row in 6..24 {
            let off = row * 4;
            assert_eq!(PATTERN_P25[off + 3], 0, "row {row} should be transparent");
        }
    }

    #[test]
    fn pattern_p50_has_12_filled_rows() {
        assert_eq!(PATTERN_P50[11 * 4 + 3], 80);
        assert_eq!(PATTERN_P50[12 * 4 + 3], 0);
    }

    #[test]
    fn pattern_p75_has_18_filled_rows() {
        assert_eq!(PATTERN_P75[17 * 4 + 3], 80);
        assert_eq!(PATTERN_P75[18 * 4 + 3], 0);
    }

    #[test]
    fn select_overflow_pattern_fractions() {
        assert_eq!(select_overflow_pattern(7, 28), PartialPattern::P25);
        assert_eq!(select_overflow_pattern(14, 28), PartialPattern::P50);
        assert_eq!(select_overflow_pattern(21, 28), PartialPattern::P75);
        assert_eq!(select_overflow_pattern(25, 28), PartialPattern::Full);
    }

    #[test]
    fn select_overflow_pattern_zero_ch() {
        assert_eq!(select_overflow_pattern(5, 0), PartialPattern::Full);
    }
}
