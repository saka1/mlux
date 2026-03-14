//! Kitty image tile cache, redraw orchestration, and prefetch.

use log::debug;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::mpsc;

use super::layout::{Layout, ScrollState};
use super::terminal;
use crate::highlight::HighlightSpec;
use crate::tile::{DocumentMeta, TilePngs, TiledDocumentCache, VisibleTiles};

/// Request sent to the prefetch worker thread.
pub(super) struct TileRequest {
    pub idx: usize,
    pub highlight: Option<HighlightSpec>,
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
    evict_distance: usize,
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
            .filter_map(|k| self.map.remove(&k).map(|ids| (k, ids)))
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
        req_tx: &mpsc::Sender<TileRequest>,
        res_rx: &mpsc::Receiver<(usize, TilePngs)>,
        in_flight: &mut HashSet<usize>,
        highlight: Option<&HighlightSpec>,
    ) -> anyhow::Result<()> {
        if let Some(action) = self.plan_load(idx) {
            if !cache.contains(idx) {
                if in_flight.insert(idx) {
                    let _ = req_tx.send(TileRequest {
                        idx,
                        highlight: highlight.cloned(),
                    });
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
    pub req_tx: &'a mpsc::Sender<TileRequest>,
    pub res_rx: &'a mpsc::Receiver<(usize, TilePngs)>,
    pub in_flight: &'a mut HashSet<usize>,
}

/// Full redraw: content tiles + sidebar + status bar.
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
    highlight: Option<&HighlightSpec>,
) -> anyhow::Result<()> {
    let visible = meta.visible_tiles(scroll.y_offset, scroll.vp_h);

    // Phase 1: Ensure all needed tiles are rendered and sent to the terminal.
    match &visible {
        VisibleTiles::Single { idx, .. } => {
            loaded.ensure_loaded(cache, *idx, pf.req_tx, pf.res_rx, pf.in_flight, highlight)?;
        }
        VisibleTiles::Split {
            top_idx, bot_idx, ..
        } => {
            loaded.ensure_loaded(
                cache,
                *top_idx,
                pf.req_tx,
                pf.res_rx,
                pf.in_flight,
                highlight,
            )?;
            loaded.ensure_loaded(
                cache,
                *bot_idx,
                pf.req_tx,
                pf.res_rx,
                pf.in_flight,
                highlight,
            )?;
        }
    }

    // Phase 2: Delete old placements atomically, then place new ones.
    loaded.delete_placements()?;

    // Phase 3: Place content + sidebar + status bar
    terminal::place_content_tiles(&visible, loaded, layout, scroll)?;
    terminal::place_sidebar_tiles(&visible, loaded, meta.sidebar_width_px, layout)?;
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
    tx: &mpsc::Sender<TileRequest>,
    meta: &DocumentMeta,
    cache: &TiledDocumentCache,
    in_flight: &mut HashSet<usize>,
    y_offset: u32,
    highlight: Option<&HighlightSpec>,
) {
    let current = (y_offset / meta.tile_height_px) as usize;
    // Forward 2 + backward 1
    for idx in [current + 1, current + 2, current.wrapping_sub(1)] {
        if idx < meta.tile_count && !cache.contains(idx) && !in_flight.contains(&idx) {
            debug!("prefetch: requesting tile {idx} (current={current})");
            let _ = tx.send(TileRequest {
                idx,
                highlight: highlight.cloned(),
            });
            in_flight.insert(idx);
        }
    }
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
}
