//! Terminal layout geometry, scroll state, and viewport math.

use crate::tile::VisualLine;

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
/// Groups the data needed for scroll-bound calculations: current position
/// (`y_offset`), document height (`img_h`), and viewport dimensions
/// (`vp_w`, `vp_h`).
pub(super) struct ScrollState {
    pub y_offset: u32, // スクロールオフセット（ピクセル）
    pub img_h: u32,    // ドキュメント高さ（ピクセル）
    pub vp_w: u32,     // ビューポート幅（ピクセル）
    pub vp_h: u32,     // ビューポート高さ（ピクセル）
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
    let cell_w = if term_cols > 0 {
        pixel_w / term_cols
    } else {
        1
    };
    let cell_h = if term_rows > 0 {
        pixel_h / term_rows
    } else {
        1
    };
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
                md_line_range: None,
                md_line_exact: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 100,
                md_line_range: None,
                md_line_exact: None,
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
                md_line_range: None,
                md_line_exact: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 100,
                md_line_range: None,
                md_line_exact: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 200,
                md_line_range: None,
                md_line_exact: None,
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
            md_line_range: None,
            md_line_exact: None,
        }];
        assert_eq!(visual_line_offset(&vls, 1000, 99), 0);
    }

    #[test]
    fn visual_line_offset_clamps_to_max() {
        let vls = vec![
            VisualLine {
                y_pt: 0.0,
                y_px: 0,
                md_line_range: None,
                md_line_exact: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 500,
                md_line_range: None,
                md_line_exact: None,
            },
        ];
        // Line 2 → idx 1, previous idx 0 → y_px 0, min(100) → 0
        assert_eq!(visual_line_offset(&vls, 100, 2), 0);
        // With high y_px that exceeds max_scroll
        let vls2 = vec![
            VisualLine {
                y_pt: 0.0,
                y_px: 0,
                md_line_range: None,
                md_line_exact: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 9999,
                md_line_range: None,
                md_line_exact: None,
            },
            VisualLine {
                y_pt: 0.0,
                y_px: 10000,
                md_line_range: None,
                md_line_exact: None,
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
}
