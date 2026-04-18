//! Terminal layout geometry, scroll state, and viewport math.

use std::time::Duration;

use super::scroll_animator::ScrollAnimator;
use crate::frame::VisualLine;

// ---------------------------------------------------------------------------
// Layout / ScrollState
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub sidebar_cols: u16,
    pub image_col: u16,  // 画像領域の開始列 (= sidebar_cols)
    pub image_cols: u16, // 画像領域の幅 (= term_cols - sidebar_cols)
    pub image_rows: u16, // 画像領域の高さ (= term_rows - 1)
    pub status_row: u16, // ステータスバーの行 (= term_rows - 1)
    pub cell_w: u16,     // ピクセル/セル（幅）
    pub cell_h: u16,     // ピクセル/セル（高さ）
}

/// Scroll position and viewport/document pixel dimensions.
///
/// Groups the data needed for scroll-bound calculations: current rendered
/// position (`y_offset`), desired position (`target_y`), document height
/// (`img_h`), viewport dimensions (`vp_w`, `vp_h`), and the animation
/// strategy driving `y_offset` toward `target_y` each frame.
///
/// The animator owns the sub-pixel position; `y_offset` is the rounded
/// integer readers depend on.
pub(super) struct ScrollState {
    /// Rendered integer position (pixels). Updated by `tick()` from the
    /// animator's current sub-pixel position. Read widely by downstream
    /// code (status bar, visible_tiles, prefetch, etc.).
    pub y_offset: u32,
    /// Desired final position set by `Effect::ScrollTo`.
    pub target_y: u32,
    pub img_h: u32, // ドキュメント高さ（ピクセル）
    pub vp_w: u32,  // ビューポート幅（ピクセル）
    pub vp_h: u32,  // ビューポート高さ（ピクセル）
    /// Animation strategy. Advances `y_offset` toward `target_y` each tick.
    pub animator: ScrollAnimator,
}

impl ScrollState {
    /// Construct a `ScrollState` at rest at `initial_y` (no pending animation).
    pub fn new(
        initial_y: u32,
        img_h: u32,
        vp_w: u32,
        vp_h: u32,
        animation: crate::config::ScrollAnimation,
    ) -> Self {
        Self {
            y_offset: initial_y,
            target_y: initial_y,
            img_h,
            vp_w,
            vp_h,
            animator: ScrollAnimator::from_config(initial_y as f64, animation),
        }
    }

    /// Whether an in-flight animation is still running.
    pub fn is_animating(&self) -> bool {
        self.animator.is_animating(self.target_y as f64)
    }

    /// Advance the animation one frame. Returns true if `y_offset` (the
    /// rendered integer position) changed — callers use this to decide
    /// whether to redraw.
    pub fn tick(&mut self, dt: Duration) -> bool {
        let prev = self.y_offset;
        let current = self.animator.tick(self.target_y as f64, dt);
        self.y_offset = current.round() as u32;
        self.y_offset != prev
    }
}

impl Layout {
    /// Viewport width in typst points.
    pub(super) fn viewport_width_pt(&self, ppi: f64) -> f64 {
        self.image_cols as f64 * self.cell_w as f64 * 72.0 / ppi
    }

    /// Viewport height in typst points.
    pub(super) fn viewport_height_pt(&self, ppi: f64) -> f64 {
        self.image_rows as f64 * self.cell_h as f64 * 72.0 / ppi
    }

    /// Sidebar width in typst points.
    pub(super) fn sidebar_width_pt(&self, ppi: f64) -> f64 {
        self.sidebar_cols as f64 * self.cell_w as f64 * 72.0 / ppi
    }

    /// Align tile height (pt) to cell_h pixel boundary, ensuring exact 1:1 scaling.
    pub(super) fn align_tile_height_pt(&self, tile_height_pt: f64, ppi: f64) -> f64 {
        let raw_px = (tile_height_pt * ppi / 72.0).round() as u32;
        let cell_h = self.cell_h as u32;
        let aligned_px = raw_px.div_ceil(cell_h) * cell_h;
        aligned_px as f64 * 72.0 / ppi
    }
}

pub(super) fn compute_layout(
    term_cols: u16,
    term_rows: u16,
    pixel_w: u16,
    pixel_h: u16,
    sidebar_cols: u16,
) -> Layout {
    let image_col = sidebar_cols;
    let image_cols = term_cols.saturating_sub(sidebar_cols);
    let image_rows = term_rows.saturating_sub(1);
    let status_row = term_rows.saturating_sub(1);
    let cell_w = pixel_w.checked_div(term_cols).unwrap_or(1);
    let cell_h = pixel_h.checked_div(term_rows).unwrap_or(1);
    Layout {
        sidebar_cols,
        image_col,
        image_cols,
        image_rows,
        status_row,
        cell_w,
        cell_h,
    }
}

pub(super) fn vp_dims(layout: &Layout, img_w: u32, img_h: u32) -> (u32, u32) {
    let vp_w = (layout.image_cols as u32 * layout.cell_w as u32).min(img_w);
    let vp_h = (layout.image_rows as u32 * layout.cell_h as u32).min(img_h);
    (vp_w, vp_h)
}

/// Compute the y_offset for a 1-based visual line number (pure function, no mutation).
pub(super) fn visual_line_offset(
    visual_lines: &[VisualLine],
    max_scroll: u32,
    line_num: u32,
) -> u32 {
    let idx = (line_num as usize).saturating_sub(1); // 1-based to 0-based
    if idx < visual_lines.len() {
        // Use the previous line's baseline as the scroll target so that line N
        // appears fully visible at the top (y_px is the baseline, not the ascender).
        visual_lines[idx.saturating_sub(1)].y_px.min(max_scroll)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_layout_basic() {
        let l = compute_layout(80, 24, 1280, 576, 6);
        assert_eq!(l.sidebar_cols, 6);
        assert_eq!(l.image_col, 6);
        assert_eq!(l.image_cols, 74);
        assert_eq!(l.image_rows, 23);
        assert_eq!(l.status_row, 23);
        assert_eq!(l.cell_w, 16); // 1280/80
        assert_eq!(l.cell_h, 24); // 576/24
    }

    #[test]
    fn compute_layout_zero_cols_no_panic() {
        let l = compute_layout(0, 0, 0, 0, 0);
        assert_eq!(l.image_cols, 0);
        assert_eq!(l.cell_w, 1); // fallback
        assert_eq!(l.cell_h, 1);
    }

    #[test]
    fn visual_line_offset_first_line() {
        let vls = vec![
            VisualLine {
                y_pt: 0.0,
                y_px: 0,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 100,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
        ];
        // Line 1 → idx 0, saturating_sub(1) → idx 0 → y_px 0
        assert_eq!(visual_line_offset(&vls, 1000, 1), 0);
    }

    #[test]
    fn visual_line_offset_middle_line() {
        let vls = vec![
            VisualLine {
                y_pt: 0.0,
                y_px: 0,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 100,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 200,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
        ];
        // Line 3 → idx 2, previous line idx 1 → y_px 100
        assert_eq!(visual_line_offset(&vls, 1000, 3), 100);
    }

    #[test]
    fn visual_line_offset_out_of_range() {
        let vls = vec![VisualLine {
            y_pt: 0.0,
            y_px: 0,
            md_block_range: None,
            md_offset: None,
            diff_status: None,
        }];
        assert_eq!(visual_line_offset(&vls, 1000, 99), 0);
    }

    #[test]
    fn visual_line_offset_clamps_to_max() {
        let vls = vec![
            VisualLine {
                y_pt: 0.0,
                y_px: 0,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 500,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
        ];
        // Line 2 → idx 1, previous idx 0 → y_px 0, min(100) → 0
        assert_eq!(visual_line_offset(&vls, 100, 2), 0);
        // With high y_px that exceeds max_scroll
        let vls2 = vec![
            VisualLine {
                y_pt: 0.0,
                y_px: 0,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 9999,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 10000,
                md_block_range: None,
                md_offset: None,
                diff_status: None,
            },
        ];
        // Line 3 → idx 2, previous idx 1 → y_px 9999, min(500) → 500
        assert_eq!(visual_line_offset(&vls2, 500, 3), 500);
    }

    #[test]
    fn vp_dims_viewport_smaller_than_image() {
        let l = compute_layout(80, 24, 1280, 576, 6);
        let (vp_w, vp_h) = vp_dims(&l, 2000, 5000);
        // image_cols=74, cell_w=16 → 74*16=1184 < 2000
        assert_eq!(vp_w, 1184);
        // image_rows=23, cell_h=24 → 23*24=552 < 5000
        assert_eq!(vp_h, 552);
    }

    #[test]
    fn vp_dims_viewport_larger_than_image() {
        let l = compute_layout(80, 24, 1280, 576, 6);
        let (vp_w, vp_h) = vp_dims(&l, 100, 200);
        assert_eq!(vp_w, 100);
        assert_eq!(vp_h, 200);
    }

    #[test]
    fn layout_pt_conversions() {
        // 80x24 terminal, 1280x576 pixels → cell_w=16, cell_h=24
        // image_cols=74, image_rows=23, sidebar_cols=6
        let l = compute_layout(80, 24, 1280, 576, 6);
        let ppi = 144.0;

        // viewport_width_pt: 74 * 16 * 72 / 144 = 1184 * 0.5 = 592.0
        assert_eq!(l.viewport_width_pt(ppi), 592.0);

        // viewport_height_pt: 23 * 24 * 72 / 144 = 552 * 0.5 = 276.0
        assert_eq!(l.viewport_height_pt(ppi), 276.0);

        // sidebar_width_pt: 6 * 16 * 72 / 144 = 96 * 0.5 = 48.0
        assert_eq!(l.sidebar_width_pt(ppi), 48.0);
    }

    #[test]
    fn align_tile_height_rounds_up_to_cell_boundary() {
        let l = compute_layout(80, 24, 1280, 576, 6);
        let ppi = 144.0;
        // cell_h = 24px

        // 276pt → 276 * 144 / 72 = 552px → 552 / 24 = 23.0 → already aligned → 552px → 276pt
        assert_eq!(l.align_tile_height_pt(276.0, ppi), 276.0);

        // 277pt → 277 * 144 / 72 = 554px → div_ceil(554, 24) = 24 → 24*24 = 576px → 288pt
        assert_eq!(l.align_tile_height_pt(277.0, ppi), 288.0);
    }
}
