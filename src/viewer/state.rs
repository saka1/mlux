//! Application state: layout, viewport, loaded tiles, redraw, prefetch.

use log::debug;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::mpsc;

use super::terminal;
use crate::tile::{TiledDocument, TiledDocumentCache, VisibleTiles, VisualLine};

// ---------------------------------------------------------------------------
// Layout / ViewState
// ---------------------------------------------------------------------------

pub(super) struct Layout {
    pub sidebar_cols: u16,
    pub image_col: u16,    // 画像領域の開始列 (= sidebar_cols)
    pub image_cols: u16,   // 画像領域の幅 (= term_cols - sidebar_cols)
    pub image_rows: u16,   // 画像領域の高さ (= term_rows - 1)
    pub status_row: u16,   // ステータスバーの行 (= term_rows - 1)
    pub cell_w: u16,       // ピクセル/セル（幅）
    pub cell_h: u16,       // ピクセル/セル（高さ）
}

pub(super) struct ViewState {
    pub y_offset: u32,   // スクロールオフセット（ピクセル）
    pub img_h: u32,      // ドキュメント高さ（ピクセル）
    pub vp_w: u32,       // ビューポート幅（ピクセル）
    pub vp_h: u32,       // ビューポート高さ（ピクセル）
    pub filename: String,
}

pub(super) fn compute_layout(term_cols: u16, term_rows: u16, pixel_w: u16, pixel_h: u16) -> Layout {
    let sidebar_cols: u16 = 6;
    let image_col = sidebar_cols;
    let image_cols = term_cols.saturating_sub(sidebar_cols);
    let image_rows = term_rows.saturating_sub(1);
    let status_row = term_rows.saturating_sub(1);
    let cell_w = if term_cols > 0 { pixel_w / term_cols } else { 1 };
    let cell_h = if term_rows > 0 { pixel_h / term_rows } else { 1 };
    Layout { sidebar_cols, image_col, image_cols, image_rows, status_row, cell_w, cell_h }
}

pub(super) fn vp_dims(layout: &Layout, img_w: u32, img_h: u32) -> (u32, u32) {
    let vp_w = (layout.image_cols as u32 * layout.cell_w as u32).min(img_w);
    let vp_h = (layout.image_rows as u32 * layout.cell_h as u32).min(img_h);
    (vp_w, vp_h)
}

/// Jump scroll offset so that the given 1-based visual line is near the top of the viewport.
pub(super) fn jump_to_visual_line(
    state: &mut ViewState,
    visual_lines: &[VisualLine],
    max_scroll: u32,
    line_num: u32,
) {
    let idx = (line_num as usize).saturating_sub(1); // 1-based to 0-based
    if idx < visual_lines.len() {
        state.y_offset = visual_lines[idx].y_px.min(max_scroll);
    }
}

// ---------------------------------------------------------------------------
// Tile-aware content display
// ---------------------------------------------------------------------------

/// Kitty image IDs for a content + sidebar tile pair.
pub(super) struct TileImageIds {
    pub content_id: u32,
    pub sidebar_id: u32,
}

/// Track which tile PNGs are loaded in the terminal, keyed by tile index.
pub(super) struct LoadedTiles {
    /// tile_index → Kitty image IDs (content + sidebar)
    pub map: HashMap<usize, TileImageIds>,
    next_id: u32,
}

impl LoadedTiles {
    pub(super) fn new() -> Self {
        Self {
            map: HashMap::new(),
            next_id: 100, // Reserve 1-99 for future use
        }
    }

    /// Ensure a tile (content + sidebar) is loaded in the terminal.
    pub(super) fn ensure_loaded(
        &mut self,
        tiled_doc: &TiledDocument,
        cache: &mut TiledDocumentCache,
        idx: usize,
    ) -> anyhow::Result<()> {
        if self.map.contains_key(&idx) {
            return Ok(());
        }

        let content_id = self.next_id;
        let sidebar_id = self.next_id + 1;
        self.next_id += 2;

        let pngs = cache.get_or_render(tiled_doc, idx)?;
        terminal::send_image(&pngs.content, content_id)?;
        terminal::send_image(&pngs.sidebar, sidebar_id)?;
        self.map.insert(idx, TileImageIds { content_id, sidebar_id });

        // Evict tiles far from current viewport to bound terminal memory
        let to_evict: Vec<usize> = self
            .map
            .keys()
            .filter(|&&k| (k as isize - idx as isize).unsigned_abs() > 4)
            .copied()
            .collect();
        for k in to_evict {
            if let Some(ids) = self.map.remove(&k) {
                let _ = terminal::delete_image(ids.content_id);
                let _ = terminal::delete_image(ids.sidebar_id);
            }
        }

        Ok(())
    }

    /// Delete all tile placements (content + sidebar, keep image data).
    pub(super) fn delete_placements(&self) -> io::Result<()> {
        use std::io::Write;
        let mut out = std::io::stdout();
        for ids in self.map.values() {
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ids.content_id)?;
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ids.sidebar_id)?;
        }
        out.flush()
    }
}

/// Why the event loop exited the inner `thread::scope`.
pub(super) enum ExitReason {
    Quit,
    Resize { new_cols: u16, new_rows: u16 },
}

/// Full redraw: content tiles + sidebar + status bar.
///
/// Ordering: ensure loaded (slow) → delete placements → place new (fast).
pub(super) fn redraw(
    tiled_doc: &TiledDocument,
    cache: &mut TiledDocumentCache,
    loaded: &mut LoadedTiles,
    layout: &Layout,
    state: &ViewState,
    acc_peek: Option<u32>,
    flash: Option<&str>,
) -> anyhow::Result<()> {
    let visible = tiled_doc.visible_tiles(state.y_offset, state.vp_h);

    // Phase 1: Ensure all needed tiles are rendered and sent to the terminal.
    match &visible {
        VisibleTiles::Single { idx, .. } => {
            loaded.ensure_loaded(tiled_doc, cache, *idx)?;
        }
        VisibleTiles::Split { top_idx, bot_idx, .. } => {
            loaded.ensure_loaded(tiled_doc, cache, *top_idx)?;
            loaded.ensure_loaded(tiled_doc, cache, *bot_idx)?;
        }
    }

    // Phase 2: Delete old placements atomically, then place new ones.
    loaded.delete_placements()?;

    // Phase 3: Place content + sidebar + status bar
    terminal::place_content_tiles(&visible, loaded, layout, state)?;
    terminal::place_sidebar_tiles(&visible, loaded, tiled_doc, layout)?;
    terminal::draw_status_bar(layout, state, acc_peek, flash)?;
    Ok(())
}

/// Request prefetch of tiles adjacent to the current viewport.
///
/// Sends tile indices for 2 tiles ahead and 1 behind the current position.
///
/// ## in_flight による二重レンダリング防止
///
/// `cache` だけでは TOCTOU (Time-of-Check-to-Time-of-Use) が発生する:
///   1. worker がタイル N をレンダリング完了 → `res_tx.send()` で結果送信
///   2. main thread の `send_prefetch()` が `cache.contains(N)` を検査 → false
///      (結果は mpsc チャネル内にあるが、まだ `cache.insert()` されていない)
///   3. タイル N を再リクエスト → worker が同じタイルを二重レンダリング
///
/// `in_flight` は「送信済み・未受信」のタイル index を追跡し、この隙間を埋める:
///   - `send_prefetch()`: `in_flight` に insert してからリクエスト送信
///   - `res_rx.try_recv()`: 結果受信時に `in_flight` から remove
///
/// `in_flight` は main thread 専用。worker thread はアクセスしない。
pub(super) fn send_prefetch(
    tx: &mpsc::Sender<usize>,
    doc: &TiledDocument,
    cache: &TiledDocumentCache,
    in_flight: &mut HashSet<usize>,
    y_offset: u32,
) {
    let current = (y_offset / doc.tile_height_px()) as usize;
    // Forward 2 + backward 1
    for idx in [current + 1, current + 2, current.wrapping_sub(1)] {
        if idx < doc.tile_count() && !cache.contains(idx) && !in_flight.contains(&idx) {
            debug!("prefetch: requesting tile {idx} (current={current})");
            let _ = tx.send(idx);
            in_flight.insert(idx);
        }
    }
}
