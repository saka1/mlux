//! Word-level search highlighting for rendered tile PNGs.
//!
//! Walks the Typst Frame tree to find TextItems matching a regex pattern,
//! computes pixel rectangles for matching glyphs, and draws semi-transparent
//! highlight overlays on the tile's Pixmap before PNG encoding.

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
#[derive(Debug)]
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
        let abs_y_pt = abs_pos.y.to_pt() - text_height_pt * 0.8;
        let width_pt = max_x_pt - min_x_pt;

        let x_px = (abs_x_pt * pixel_per_pt as f64).round().max(0.0) as u32;
        let y_px = (abs_y_pt * pixel_per_pt as f64).round().max(0.0) as u32;
        let w_px = (width_pt * pixel_per_pt as f64).round().max(1.0) as u32;
        let h_px = (text_height_pt * pixel_per_pt as f64).round().max(1.0) as u32;

        rects.push(HighlightRect {
            x_px,
            y_px,
            w_px,
            h_px,
        });
    }
}

/// Draw semi-transparent highlight rectangles on a Pixmap.
///
/// Uses alpha blending: highlight color is mixed with the existing pixel.
pub fn draw_highlights(pixmap: &mut tiny_skia::Pixmap, rects: &[HighlightRect]) {
    let pw = pixmap.width();
    let ph = pixmap.height();

    for rect in rects {
        let x0 = rect.x_px.min(pw);
        let y0 = rect.y_px.min(ph);
        let x1 = (rect.x_px + rect.w_px).min(pw);
        let y1 = (rect.y_px + rect.h_px).min(ph);

        if x0 >= x1 || y0 >= y1 {
            continue;
        }

        // Yellow highlight: RGBA(255, 220, 0, 80) ≈ 31% opacity
        let hr = 255u8;
        let hg = 220u8;
        let hb = 0u8;
        let ha = 80u8;
        let alpha = ha as f32 / 255.0;

        let data = pixmap.data_mut();
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = (y * pw + x) as usize * 4;
                if idx + 3 < data.len() {
                    // tiny-skia stores premultiplied RGBA
                    let sr = data[idx];
                    let sg = data[idx + 1];
                    let sb = data[idx + 2];
                    let sa = data[idx + 3];

                    // "Source over" blend with highlight on top
                    let out_r = ((hr as f32 * alpha) + sr as f32 * (1.0 - alpha)).round() as u8;
                    let out_g = ((hg as f32 * alpha) + sg as f32 * (1.0 - alpha)).round() as u8;
                    let out_b = ((hb as f32 * alpha) + sb as f32 * (1.0 - alpha)).round() as u8;
                    let out_a = ((ha as f32 + sa as f32 * (1.0 - alpha)).round() as u8).max(sa);

                    data[idx] = out_r;
                    data[idx + 1] = out_g;
                    data[idx + 2] = out_b;
                    data[idx + 3] = out_a;
                }
            }
        }
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
    fn draw_highlights_on_small_pixmap() {
        let mut pixmap = tiny_skia::Pixmap::new(10, 10).unwrap();
        // Fill with white
        for px in pixmap.data_mut().chunks_exact_mut(4) {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
            px[3] = 255;
        }

        let rects = vec![HighlightRect {
            x_px: 2,
            y_px: 2,
            w_px: 3,
            h_px: 3,
        }];
        draw_highlights(&mut pixmap, &rects);

        // Check that the highlighted pixel was tinted yellow
        let idx = (2 * 10 + 2) as usize * 4;
        let data = pixmap.data();
        // After blending yellow (255,220,0,80) over white (255,255,255,255):
        // R: 255*0.31 + 255*0.69 = 255
        // G: 220*0.31 + 255*0.69 ≈ 244
        // B: 0*0.31 + 255*0.69 ≈ 176
        assert_eq!(data[idx], 255); // R stays 255
        assert!(data[idx + 1] < 255); // G reduced
        assert!(data[idx + 2] < 255); // B reduced significantly
    }

    #[test]
    fn draw_highlights_clamps_to_pixmap_bounds() {
        let mut pixmap = tiny_skia::Pixmap::new(5, 5).unwrap();
        let rects = vec![HighlightRect {
            x_px: 3,
            y_px: 3,
            w_px: 100, // way out of bounds
            h_px: 100,
        }];
        // Should not panic
        draw_highlights(&mut pixmap, &rects);
    }
}
