//! Kitty image tile cache, redraw orchestration, and prefetch.

use log::debug;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::mpsc;

use super::layout::{Layout, ScrollState};
use super::terminal;
use crate::highlight::{HighlightRect, HighlightSpec};
use crate::tile::{DocumentMeta, TilePngs, TiledDocumentCache, VisibleTiles};

/// Request sent to the prefetch worker thread.
pub(super) enum WorkerRequest {
    /// Render a base tile (content + sidebar).
    RenderTile(usize),
    /// Compute highlight rectangles for a tile.
    FindRects { idx: usize, spec: HighlightSpec },
}

// ---------------------------------------------------------------------------
// Tile-aware content display
// ---------------------------------------------------------------------------

/// Kitty image IDs for a content + sidebar tile pair.
pub(super) struct TileImageIds {
    pub content_id: u32,
    pub sidebar_id: u32,
}

/// KGP image IDs for all highlight images (full-width PNG + partial patterns).
pub(super) struct HighlightImages {
    /// 2048×24 yellow PNG for precise width cropping (non-active matches).
    pub full_id: u32,
    /// 1×24 RGBA: top 75% yellow.
    pub p75_id: u32,
    /// 1×24 RGBA: top 50% yellow.
    pub p50_id: u32,
    /// 1×24 RGBA: top 25% yellow.
    pub p25_id: u32,
    /// 2048×24 orange PNG for the active match.
    pub active_full_id: u32,
    /// 1×24 RGBA: top 75% orange (active match).
    pub active_p75_id: u32,
    /// 1×24 RGBA: top 50% orange (active match).
    pub active_p50_id: u32,
    /// 1×24 RGBA: top 25% orange (active match).
    pub active_p25_id: u32,
}

impl HighlightImages {
    /// All image IDs for bulk operations (placement deletion, etc.).
    fn all_ids(&self) -> [u32; 8] {
        [
            self.full_id,
            self.p75_id,
            self.p50_id,
            self.p25_id,
            self.active_full_id,
            self.active_p75_id,
            self.active_p50_id,
            self.active_p25_id,
        ]
    }
}

/// Track which tile PNGs are loaded in the terminal, keyed by tile index.
pub(super) struct LoadedTiles {
    /// tile_index → Kitty image IDs (content + sidebar)
    pub map: HashMap<usize, TileImageIds>,
    next_id: u32,
    evict_distance: usize,
    /// Highlight rectangles per tile, populated by `update_overlays`.
    overlay_rects: HashMap<usize, Vec<HighlightRect>>,
    /// KGP image IDs for highlight images (uploaded once).
    highlight_images: Option<HighlightImages>,
}

/// Describes the actions needed to load a tile into the terminal.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct LoadAction {
    pub idx: usize,
    pub content_id: u32,
    pub sidebar_id: u32,
    pub evict: Vec<(usize, TileImageIds)>,
}

impl LoadedTiles {
    pub(super) fn new(evict_distance: usize) -> Self {
        Self {
            map: HashMap::new(),
            next_id: 100, // Reserve 1-99 for future use
            evict_distance,
            overlay_rects: HashMap::new(),
            highlight_images: None,
        }
    }

    /// Plan what needs to happen to load a tile. Returns `None` if already loaded.
    ///
    /// This is pure: it allocates IDs, computes eviction targets, and updates
    /// the internal map, but performs no I/O. Call `execute_load()` to actually
    /// send images to the terminal.
    pub(super) fn plan_load(&mut self, idx: usize) -> Option<LoadAction> {
        if self.map.contains_key(&idx) {
            return None;
        }

        let content_id = self.next_id;
        let sidebar_id = self.next_id + 1;
        self.next_id += 2;

        self.map.insert(
            idx,
            TileImageIds {
                content_id,
                sidebar_id,
            },
        );

        // Compute eviction targets (tiles far from current viewport)
        let to_evict: Vec<usize> = self
            .map
            .keys()
            .filter(|&&k| (k as isize - idx as isize).unsigned_abs() > self.evict_distance)
            .copied()
            .collect();
        let evict = to_evict
            .into_iter()
            .filter_map(|k| {
                self.overlay_rects.remove(&k);
                self.map.remove(&k).map(|ids| (k, ids))
            })
            .collect();

        Some(LoadAction {
            idx,
            content_id,
            sidebar_id,
            evict,
        })
    }

    /// Ensure a tile (content + sidebar) is loaded in the terminal.
    ///
    /// If the tile is not in the doc cache, sends a request to the prefetch worker
    /// and blocks until the result arrives.
    pub(super) fn ensure_loaded(
        &mut self,
        cache: &mut TiledDocumentCache,
        idx: usize,
        req_tx: &mpsc::Sender<WorkerRequest>,
        res_rx: &mpsc::Receiver<(usize, TilePngs)>,
        in_flight: &mut HashSet<usize>,
    ) -> anyhow::Result<()> {
        if let Some(action) = self.plan_load(idx) {
            if !cache.contains(idx) {
                if in_flight.insert(idx) {
                    let _ = req_tx.send(WorkerRequest::RenderTile(idx));
                }
                while !cache.contains(idx) {
                    let (i, pngs) = res_rx.recv()?;
                    in_flight.remove(&i);
                    cache.insert(i, pngs);
                }
            }
            execute_load(&action, cache.get(idx).unwrap())?;
        }
        Ok(())
    }

    /// Store highlight rectangles for a tile.
    pub(super) fn set_overlay_rects(&mut self, idx: usize, rects: Vec<HighlightRect>) {
        self.overlay_rects.insert(idx, rects);
    }

    /// Get highlight rectangles for a tile.
    pub(super) fn overlay_rects(&self, idx: usize) -> &[HighlightRect] {
        self.overlay_rects.get(&idx).map_or(&[], |v| v.as_slice())
    }

    /// Whether this tile already has overlay rects computed.
    pub(super) fn has_overlay(&self, idx: usize) -> bool {
        self.overlay_rects.contains_key(&idx)
    }

    /// Ensure all highlight images (full PNG + partial patterns) are uploaded.
    pub(super) fn ensure_highlight_images(&mut self) -> io::Result<&HighlightImages> {
        if let Some(ref imgs) = self.highlight_images {
            return Ok(imgs);
        }
        let base = self.next_id;
        self.next_id += 8;

        use crate::highlight::{PATTERN_HEIGHT, PATTERN_WIDTH};

        // Yellow (non-active) images
        terminal::send_image(crate::highlight::HIGHLIGHT_PNG, base)?;
        for (i, pattern) in [
            &crate::highlight::PATTERN_P75,
            &crate::highlight::PATTERN_P50,
            &crate::highlight::PATTERN_P25,
        ]
        .iter()
        .enumerate()
        {
            terminal::send_raw_image(*pattern, PATTERN_WIDTH, PATTERN_HEIGHT, base + 1 + i as u32)?;
        }

        // Orange (active) images
        terminal::send_image(crate::highlight::HIGHLIGHT_ACTIVE_PNG, base + 4)?;
        for (i, pattern) in [
            &crate::highlight::PATTERN_ACTIVE_P75,
            &crate::highlight::PATTERN_ACTIVE_P50,
            &crate::highlight::PATTERN_ACTIVE_P25,
        ]
        .iter()
        .enumerate()
        {
            terminal::send_raw_image(*pattern, PATTERN_WIDTH, PATTERN_HEIGHT, base + 5 + i as u32)?;
        }

        self.highlight_images = Some(HighlightImages {
            full_id: base,
            p75_id: base + 1,
            p50_id: base + 2,
            p25_id: base + 3,
            active_full_id: base + 4,
            active_p75_id: base + 5,
            active_p50_id: base + 6,
            active_p25_id: base + 7,
        });
        Ok(self.highlight_images.as_ref().unwrap())
    }

    /// Get the highlight images (if uploaded).
    pub(super) fn highlight_images(&self) -> Option<&HighlightImages> {
        self.highlight_images.as_ref()
    }

    /// Reset all state after `delete_all_images()`. Both tile map and
    /// highlight images must be re-uploaded since the terminal no longer
    /// has any image data.
    pub(super) fn clear_all(&mut self) {
        self.map.clear();
        self.highlight_images = None;
    }

    /// Clear overlay rect state only (no I/O).
    pub(super) fn clear_overlay_state(&mut self) {
        self.overlay_rects.clear();
    }

    /// Delete highlight overlay placements from terminal (I/O only).
    pub(super) fn delete_overlay_placements(&self) -> io::Result<()> {
        if let Some(imgs) = &self.highlight_images {
            delete_placements_for_ids(&imgs.all_ids())?;
        }
        Ok(())
    }

    /// Clear overlay state, keeping base tiles intact.
    pub(super) fn clear_overlays(&mut self) -> io::Result<()> {
        self.clear_overlay_state();
        self.delete_overlay_placements()
    }

    /// Delete all tile placements (content + sidebar + highlight overlay).
    pub(super) fn delete_placements(&self) -> io::Result<()> {
        use std::io::Write;
        let mut out = std::io::stdout();
        for ids in self.map.values() {
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ids.content_id)?;
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ids.sidebar_id)?;
        }
        if let Some(imgs) = &self.highlight_images {
            for id in imgs.all_ids() {
                write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")?;
            }
        }
        out.flush()
    }
}

/// Delete placements for a set of image IDs.
fn delete_placements_for_ids(ids: &[u32]) -> io::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout();
    for &id in ids {
        write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")?;
    }
    out.flush()
}

/// Execute the I/O for a load action: send images to the terminal and evict distant tiles.
fn execute_load(action: &LoadAction, pngs: &crate::tile::TilePngs) -> anyhow::Result<()> {
    terminal::send_image(&pngs.content, action.content_id)?;
    terminal::send_image(&pngs.sidebar, action.sidebar_id)?;
    for (_, ids) in &action.evict {
        let _ = terminal::delete_image(ids.content_id);
        let _ = terminal::delete_image(ids.sidebar_id);
    }
    Ok(())
}

/// Prefetch channel handles for requesting and receiving rendered tiles.
pub(super) struct PrefetchChannels<'a> {
    pub req_tx: &'a mpsc::Sender<WorkerRequest>,
    pub res_rx: &'a mpsc::Receiver<(usize, TilePngs)>,
    pub in_flight: &'a mut HashSet<usize>,
}

/// Full redraw: content tiles + sidebar + overlay + status bar.
///
/// Ordering: ensure loaded (slow) → delete placements → place new (fast).
#[allow(clippy::too_many_arguments)]
pub(super) fn redraw(
    meta: &DocumentMeta,
    cache: &mut TiledDocumentCache,
    loaded: &mut LoadedTiles,
    layout: &Layout,
    scroll: &ScrollState,
    filename: &str,
    acc_peek: Option<u32>,
    flash: Option<&str>,
    pf: &mut PrefetchChannels<'_>,
) -> anyhow::Result<()> {
    let visible = meta.visible_tiles(scroll.y_offset, scroll.vp_h);

    // Phase 1: Ensure all needed tiles are rendered and sent to the terminal.
    match &visible {
        VisibleTiles::Single { idx, .. } => {
            loaded.ensure_loaded(cache, *idx, pf.req_tx, pf.res_rx, pf.in_flight)?;
        }
        VisibleTiles::Split {
            top_idx, bot_idx, ..
        } => {
            loaded.ensure_loaded(cache, *top_idx, pf.req_tx, pf.res_rx, pf.in_flight)?;
            loaded.ensure_loaded(cache, *bot_idx, pf.req_tx, pf.res_rx, pf.in_flight)?;
        }
    }

    // Phase 2: Delete old placements atomically, then place new ones.
    loaded.delete_placements()?;

    // Phase 3: Place content + sidebar + overlay + status bar
    terminal::place_content_tiles(&visible, loaded, layout, scroll)?;
    terminal::place_sidebar_tiles(&visible, loaded, meta.sidebar_width_px, layout)?;
    terminal::place_overlay_rects(&visible, loaded, layout)?;
    terminal::draw_status_bar(layout, scroll, filename, acc_peek, flash)?;
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
    tx: &mpsc::Sender<WorkerRequest>,
    meta: &DocumentMeta,
    cache: &TiledDocumentCache,
    in_flight: &mut HashSet<usize>,
    y_offset: u32,
) {
    let current = (y_offset / meta.tile_height_px) as usize;
    // Forward 2 + backward 1
    for idx in [current + 1, current + 2, current.wrapping_sub(1)] {
        if idx < meta.tile_count && !cache.contains(idx) && !in_flight.contains(&idx) {
            debug!("prefetch: requesting tile {idx} (current={current})");
            let _ = tx.send(WorkerRequest::RenderTile(idx));
            in_flight.insert(idx);
        }
    }
}

/// Update search highlight overlays for visible tiles.
///
/// Sends `FindRects` requests to the worker for tiles that haven't been
/// computed yet, receives rects, and stores them for placement by
/// `place_overlay_rects`.
pub(super) fn update_overlays(
    meta: &DocumentMeta,
    loaded: &mut LoadedTiles,
    scroll: &ScrollState,
    spec: &HighlightSpec,
    req_tx: &mpsc::Sender<WorkerRequest>,
    rect_rx: &mpsc::Receiver<(usize, Vec<HighlightRect>)>,
) -> anyhow::Result<()> {
    let visible = meta.visible_tiles(scroll.y_offset, scroll.vp_h);

    let indices: Vec<usize> = match &visible {
        VisibleTiles::Single { idx, .. } => vec![*idx],
        VisibleTiles::Split {
            top_idx, bot_idx, ..
        } => vec![*top_idx, *bot_idx],
    };

    let mut needed = 0;
    for &idx in &indices {
        if loaded.has_overlay(idx) {
            continue;
        }
        let _ = req_tx.send(WorkerRequest::FindRects {
            idx,
            spec: spec.clone(),
        });
        needed += 1;
    }

    while needed > 0 {
        let (idx, rects) = rect_rx.recv()?;
        needed -= 1;
        loaded.set_overlay_rects(idx, rects);
    }

    // Ensure the shared highlight images are uploaded.
    if indices
        .iter()
        .any(|idx| !loaded.overlay_rects(*idx).is_empty())
    {
        loaded.ensure_highlight_images()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_load_allocates_ids() {
        let mut loaded = LoadedTiles::new(3);
        let action = loaded.plan_load(0).unwrap();
        assert_eq!(action.idx, 0);
        assert_eq!(action.content_id, 100);
        assert_eq!(action.sidebar_id, 101);
        assert!(action.evict.is_empty());
    }

    #[test]
    fn plan_load_already_loaded_returns_none() {
        let mut loaded = LoadedTiles::new(3);
        loaded.plan_load(0); // load tile 0
        assert!(loaded.plan_load(0).is_none());
    }

    #[test]
    fn plan_load_evicts_distant_tiles() {
        let mut loaded = LoadedTiles::new(2); // evict_distance = 2
        loaded.plan_load(0);
        loaded.plan_load(1);
        loaded.plan_load(2);
        // Loading tile 5: distance from 0 is 5 > 2, should evict tile 0
        let action = loaded.plan_load(5).unwrap();
        assert!(!action.evict.is_empty());
        // Tile 0 should be evicted (distance 5 > 2)
        assert!(action.evict.iter().any(|(idx, _)| *idx == 0));
        // Tile 0 should no longer be in the map
        assert!(!loaded.map.contains_key(&0));
    }

    #[test]
    fn plan_load_increments_ids() {
        let mut loaded = LoadedTiles::new(3);
        let a1 = loaded.plan_load(0).unwrap();
        let a2 = loaded.plan_load(1).unwrap();
        assert_eq!(a1.content_id, 100);
        assert_eq!(a1.sidebar_id, 101);
        assert_eq!(a2.content_id, 102);
        assert_eq!(a2.sidebar_id, 103);
    }

    #[test]
    fn clear_overlay_state_empties_rects() {
        let mut loaded = LoadedTiles::new(3);
        loaded.set_overlay_rects(0, vec![]);
        assert!(loaded.has_overlay(0));
        loaded.clear_overlay_state();
        assert!(!loaded.has_overlay(0));
    }
}
