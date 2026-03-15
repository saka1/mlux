//! Word-level search highlighting via KGP overlay.
//!
//! Walks the Typst Frame tree to find TextItems matching a regex pattern,
//! computes pixel rectangles for matching glyphs. A static 2048×24
//! semi-transparent yellow PNG ([`HIGHLIGHT_PNG`]) is uploaded once. Its height
//! matches 12pt text at 144 PPI so KGP can place it with `r=1` and minimal
//! scaling. Source-rectangle `w` crops the width to the exact highlight span.

use std::time::Instant;

use log::debug;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use typst::layout::{Frame, FrameItem, Point};
use typst::text::TextItem;

/// Specification for what to highlight, sent via IPC to the fork child.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HighlightSpec {
    pub pattern: String,
    pub case_insensitive: bool,
}

/// A pixel-coordinate rectangle to draw as a highlight overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightRect {
    pub x_px: u32,
    pub y_px: u32,
    pub w_px: u32,
    pub h_px: u32,
}

/// Find all highlight rectangles for text matching `spec` within a tile frame.
///
/// Walks the Frame tree recursively, regex-matches against each `TextItem.text`,
/// and maps matched byte ranges to glyph pixel positions.
pub fn find_highlight_rects(frame: &Frame, spec: &HighlightSpec, ppi: f32) -> Vec<HighlightRect> {
    if spec.pattern.is_empty() {
        return Vec::new();
    }

    let start = Instant::now();

    let re = match RegexBuilder::new(&spec.pattern)
        .case_insensitive(spec.case_insensitive)
        .build()
    {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };

    let pixel_per_pt = ppi / 72.0;
    let mut rects = Vec::new();
    walk_frame(frame, Point::zero(), &re, pixel_per_pt, &mut rects);

    debug!(
        "highlight: find_highlight_rects completed in {:.1}ms ({} rects, pattern={:?})",
        start.elapsed().as_secs_f64() * 1000.0,
        rects.len(),
        spec.pattern,
    );

    rects
}

/// Recursively walk the frame tree collecting highlight rectangles.
fn walk_frame(
    frame: &Frame,
    offset: Point,
    re: &regex::Regex,
    pixel_per_pt: f32,
    rects: &mut Vec<HighlightRect>,
) {
    for (pos, item) in frame.items() {
        let abs = Point::new(offset.x + pos.x, offset.y + pos.y);
        match item {
            FrameItem::Text(text) => {
                collect_text_rects(text, abs, re, pixel_per_pt, rects);
            }
            FrameItem::Group(group) => {
                walk_frame(&group.frame, abs, re, pixel_per_pt, rects);
            }
            _ => {}
        }
    }
}

/// For a single TextItem, find regex matches and compute glyph pixel rects.
fn collect_text_rects(
    text: &TextItem,
    abs_pos: Point,
    re: &regex::Regex,
    pixel_per_pt: f32,
    rects: &mut Vec<HighlightRect>,
) {
    let text_str: &str = &text.text;
    if text_str.is_empty() {
        return;
    }

    // Precompute cumulative x positions for each glyph (in pt).
    // glyph_x_starts[i] = x offset of glyph i relative to TextItem start.
    let glyphs = &text.glyphs;
    let mut glyph_x_starts: Vec<f64> = Vec::with_capacity(glyphs.len());
    let mut cursor = 0.0_f64;
    for g in glyphs.iter() {
        glyph_x_starts.push(cursor + g.x_offset.at(text.size).to_pt());
        cursor += g.x_advance.at(text.size).to_pt();
    }

    let text_height_pt = text.size.to_pt();

    for m in re.find_iter(text_str) {
        let match_start = m.start();
        let match_end = m.end();

        // Find glyphs whose byte range overlaps the match.
        let mut min_x_pt = f64::MAX;
        let mut max_x_pt = f64::MIN;
        let mut found = false;

        for (i, g) in glyphs.iter().enumerate() {
            let g_start = g.range.start as usize;
            let g_end = g.range.end as usize;

            // Check overlap: glyph range [g_start, g_end) ∩ match range [match_start, match_end)
            if g_start < match_end && g_end > match_start {
                let x = glyph_x_starts[i];
                let w = g.x_advance.at(text.size).to_pt();
                min_x_pt = min_x_pt.min(x);
                max_x_pt = max_x_pt.max(x + w);
                found = true;
            }
        }

        if !found {
            continue;
        }

        let abs_x_pt = abs_pos.x.to_pt() + min_x_pt;
        // TextItem y position is at the baseline; shift up by ~80% of font size for top.
        let baseline_y_pt = abs_pos.y.to_pt();
        let abs_y_pt = baseline_y_pt - text_height_pt * 0.8;
        let width_pt = max_x_pt - min_x_pt;

        // Skip text whose top is above this tile's origin — it belongs to the
        // previous tile and would otherwise appear as a ghost rect at y=0.
        if abs_y_pt < 0.0 {
            continue;
        }

        let x_px = (abs_x_pt * pixel_per_pt as f64).round() as u32;
        let y_px = (abs_y_pt * pixel_per_pt as f64).round() as u32;
        let w_px = (width_pt * pixel_per_pt as f64).round().max(1.0) as u32;
        let h_px = (text_height_pt * pixel_per_pt as f64).round().max(1.0) as u32;

        debug!(
            "highlight rect: matched {:?} at baseline_y={:.2}pt abs_y={:.2}pt \
             abs_x={:.2}pt w={:.2}pt h={:.2}pt -> px({}, {}, {}, {})",
            &text_str[m.start()..m.end()],
            baseline_y_pt,
            abs_y_pt,
            abs_x_pt,
            width_pt,
            text_height_pt,
            x_px,
            y_px,
            w_px,
            h_px,
        );

        rects.push(HighlightRect {
            x_px,
            y_px,
            w_px,
            h_px,
        });
    }
}

/// 2048×24 semi-transparent yellow PNG (RGBA 255, 220, 0, 80).
///
/// Dimensions are chosen to match 12pt text at 144 PPI (= 24 px height).
/// Width is large enough for any highlight span. KGP source-rectangle `w`
/// crops to the exact highlight width; the native height already matches the
/// text line height, so KGP placement with `r=1` requires minimal scaling.
pub const HIGHLIGHT_PNG: &[u8] = include_bytes!("../assets/highlight.png");

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
    fn empty_pattern_returns_no_rects() {
        let frame = Frame::hard(typst::layout::Size::zero());
        let spec = HighlightSpec {
            pattern: String::new(),
            case_insensitive: false,
        };
        let rects = find_highlight_rects(&frame, &spec, 144.0);
        assert!(rects.is_empty());
    }

    #[test]
    fn invalid_regex_returns_no_rects() {
        let frame = Frame::hard(typst::layout::Size::zero());
        let spec = HighlightSpec {
            pattern: "[".to_string(),
            case_insensitive: false,
        };
        let rects = find_highlight_rects(&frame, &spec, 144.0);
        assert!(rects.is_empty());
    }

    #[test]
    fn highlight_png_is_valid() {
        assert!(HIGHLIGHT_PNG.len() > 8);
        assert_eq!(&HIGHLIGHT_PNG[1..4], b"PNG");
        // 256×256 image should be significantly larger than the old 1×1
        assert!(HIGHLIGHT_PNG.len() > 100);
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
