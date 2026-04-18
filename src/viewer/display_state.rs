//! Terminal display state: Kitty image cache, redraw orchestration, and prefetch.

use log::debug;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

use super::layout::{Layout, ScrollState};
use super::terminal;
use crate::frame::{DocumentMeta, HighlightRect, HighlightSpec, TileCache, VisibleTiles};
use crate::renderer::{TileRenderer, TileResponse};

/// Logical identity for a KGP placement. One `PlacementSlot` maps to at most
/// one live `a=p` on the terminal at any time, identified by
/// `(image_id, slot.placement_id())`. Re-emitting `a=p` with the same
/// `(image_id, placement_id)` is atomic in-place — no intermediate "placement
/// absent" state, so no blink on scroll.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(super) enum PlacementSlot {
    Content(usize),
    Sidebar(usize),
    OverlayPrimary(usize, usize),
    OverlayOverflow(usize, usize),
}

impl PlacementSlot {
    /// Stable KGP `p=` value. `placement_id` is scoped per-`image_id`, so
    /// Content/Sidebar can share `p=1` safely; overlays share their tile/rect
    /// image with peers, hence `2*rect_idx + {1,2}`.
    pub(super) fn placement_id(self) -> u32 {
        match self {
            PlacementSlot::Content(_) | PlacementSlot::Sidebar(_) => 1,
            PlacementSlot::OverlayPrimary(_, r) => (2 * r + 1) as u32,
            PlacementSlot::OverlayOverflow(_, r) => (2 * r + 2) as u32,
        }
    }

    pub(super) fn tile_idx(self) -> usize {
        match self {
            PlacementSlot::Content(i)
            | PlacementSlot::Sidebar(i)
            | PlacementSlot::OverlayPrimary(i, _)
            | PlacementSlot::OverlayOverflow(i, _) => i,
        }
    }

    fn is_overlay(self) -> bool {
        matches!(
            self,
            PlacementSlot::OverlayPrimary(..) | PlacementSlot::OverlayOverflow(..)
        )
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
pub(super) struct DisplayState {
    /// tile_index → Kitty image IDs (content + sidebar)
    pub map: HashMap<usize, TileImageIds>,
    next_id: u32,
    evict_distance: usize,
    /// Highlight rectangles per tile, populated by `update_overlays`.
    overlay_rects: HashMap<usize, Vec<HighlightRect>>,
    /// KGP image IDs for highlight images (uploaded once).
    highlight_images: Option<HighlightImages>,
    /// `slot → image_id` of every placement currently live on the terminal.
    /// Enables atomic in-place move: unchanged slots are re-emitted with
    /// the same `(i, p)` pair (no delete), disappeared slots are deleted
    /// individually via `a=d,d=i,i=..,p=..`.
    live_slots: HashMap<PlacementSlot, u32>,
}

/// Describes the actions needed to load a tile into the terminal.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct LoadAction {
    pub idx: usize,
    pub content_id: u32,
    pub sidebar_id: u32,
    pub evict: Vec<(usize, TileImageIds)>,
}

impl DisplayState {
    pub(super) fn new(evict_distance: usize) -> Self {
        Self::new_with_start_id(evict_distance, 100)
    }

    /// Create a DisplayState that allocates image IDs starting from `start_id`.
    ///
    /// Used by the double-buffer reload scheme: each generation starts from a
    /// fixed base (e.g. 100 or 5000) so old and new images coexist briefly.
    pub(super) fn new_with_start_id(evict_distance: usize, start_id: u32) -> Self {
        Self {
            map: HashMap::new(),
            next_id: start_id,
            evict_distance,
            overlay_rects: HashMap::new(),
            highlight_images: None,
            live_slots: HashMap::new(),
        }
    }

    /// All Kitty image IDs owned by this DisplayState (tiles + highlights).
    pub(super) fn all_image_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for tile_ids in self.map.values() {
            ids.push(tile_ids.content_id);
            ids.push(tile_ids.sidebar_id);
        }
        if let Some(ref imgs) = self.highlight_images {
            ids.extend_from_slice(&imgs.all_ids());
        }
        ids
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
                self.live_slots.retain(|slot, _| slot.tile_idx() != k);
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
    /// If the tile is not in the doc cache, sends a request to the fork child
    /// and blocks until the result arrives. Stray responses (e.g. Rects) are
    /// routed to the appropriate handler.
    pub(super) fn ensure_loaded(
        &mut self,
        cache: &mut TileCache,
        idx: usize,
        rh: &mut ForkHandle<'_>,
    ) -> anyhow::Result<()> {
        if let Some(action) = self.plan_load(idx) {
            if !cache.contains(idx) {
                if rh.in_flight.insert(idx) {
                    let _ = rh.renderer.send_render_tile(idx);
                }
                while !cache.contains(idx) {
                    match rh.renderer.recv()? {
                        TileResponse::Tile { idx: i, pngs } => {
                            rh.in_flight.remove(&i);
                            cache.insert(i, pngs);
                        }
                        TileResponse::Rects { idx: i, rects } => {
                            self.set_overlay_rects(i, rects);
                        }
                    }
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

        use crate::frame::{PATTERN_HEIGHT, PATTERN_WIDTH};

        // Yellow (non-active) images
        terminal::send_image(crate::frame::HIGHLIGHT_PNG, base)?;
        for (i, pattern) in [
            &crate::frame::PATTERN_P75,
            &crate::frame::PATTERN_P50,
            &crate::frame::PATTERN_P25,
        ]
        .iter()
        .enumerate()
        {
            terminal::send_raw_image(*pattern, PATTERN_WIDTH, PATTERN_HEIGHT, base + 1 + i as u32)?;
        }

        // Orange (active) images
        terminal::send_image(crate::frame::HIGHLIGHT_ACTIVE_PNG, base + 4)?;
        for (i, pattern) in [
            &crate::frame::PATTERN_ACTIVE_P75,
            &crate::frame::PATTERN_ACTIVE_P50,
            &crate::frame::PATTERN_ACTIVE_P25,
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

    /// Reset all state when `RenderOp::DeleteAllImages` is about to issue
    /// `terminal::delete_all_images()` in the *same* `DisplayState`'s
    /// lifetime. Both tile map and highlight images must be re-uploaded
    /// since the terminal no longer has any image data.
    ///
    /// Other `delete_all_images()` call-sites (Session resize / Navigate /
    /// GoBack) do NOT call this — they drop the `DisplayState` wholesale at
    /// the next iteration of the `'outer` loop in `viewer::run`, which
    /// achieves the same invariant by replacement rather than mutation.
    pub(super) fn clear_all(&mut self) {
        self.map.clear();
        self.highlight_images = None;
        self.live_slots.clear();
    }

    /// Clear overlay rect state only (no I/O).
    pub(super) fn clear_overlay_state(&mut self) {
        self.overlay_rects.clear();
    }

    /// Delete highlight overlay placements from terminal (I/O only).
    /// Clears overlay-kind entries from the live-slot tracker as well, since
    /// this path wipes every overlay placement regardless of slot membership.
    pub(super) fn delete_overlay_placements(&mut self) -> io::Result<()> {
        if let Some(imgs) = &self.highlight_images {
            delete_placements_for_ids(&imgs.all_ids())?;
        }
        self.live_slots.retain(|slot, _| !slot.is_overlay());
        Ok(())
    }

    /// Delete every currently-live placement (tiles + overlays), leaving
    /// uploaded image data intact. Used when a modal screen overdraws the
    /// viewer (TOC/URL/Grep/etc.) — the tiles are reused on mode exit.
    pub(super) fn delete_placements(&mut self) -> io::Result<()> {
        let mut out = std::io::stdout();
        for (slot, image_id) in self.live_slots.drain() {
            let pid = slot.placement_id();
            write!(out, "\x1b_Ga=d,d=i,i={image_id},p={pid},q=2\x1b\\")?;
        }
        out.flush()
    }

    /// Record that `slot` is now live at `image_id`. If the slot was previously
    /// live on a *different* image, emit a targeted delete for the stale
    /// placement before the caller emits the new `a=p` — Kitty's atomic
    /// in-place move only applies when the `(i, p)` pair is unchanged.
    /// Returns the `p=` value the caller should embed in its `a=p` command.
    pub(super) fn track_placement(
        &mut self,
        out: &mut impl Write,
        slot: PlacementSlot,
        image_id: u32,
    ) -> io::Result<u32> {
        let pid = slot.placement_id();
        if let Some(old_id) = self.live_slots.insert(slot, image_id)
            && old_id != image_id
        {
            write!(out, "\x1b_Ga=d,d=i,i={old_id},p={pid},q=2\x1b\\")?;
        }
        Ok(pid)
    }

    /// Emit `a=d,d=i,i=..,p=..` for every tracked slot absent from `keep`,
    /// and drop those entries from the live-slot tracker. Used in redraw
    /// Phase 2 to clear slots that no longer appear this frame.
    pub(super) fn delete_stale_slots(
        &mut self,
        out: &mut impl Write,
        keep: &HashSet<PlacementSlot>,
    ) -> io::Result<()> {
        let stale: Vec<PlacementSlot> = self
            .live_slots
            .keys()
            .filter(|s| !keep.contains(*s))
            .copied()
            .collect();
        for slot in stale {
            if let Some(image_id) = self.live_slots.remove(&slot) {
                let pid = slot.placement_id();
                write!(out, "\x1b_Ga=d,d=i,i={image_id},p={pid},q=2\x1b\\")?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn live_slot_count(&self) -> usize {
        self.live_slots.len()
    }

    #[cfg(test)]
    pub(super) fn live_slot_image(&self, slot: PlacementSlot) -> Option<u32> {
        self.live_slots.get(&slot).copied()
    }
}

/// Delete placements for a set of image IDs.
fn delete_placements_for_ids(ids: &[u32]) -> io::Result<()> {
    let mut out = std::io::stdout();
    for &id in ids {
        write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")?;
    }
    out.flush()
}

/// Collect the set of `PlacementSlot`s expected to be live this frame.
///
/// Tile slots come from `visible`. Overlay slots are enumerated from
/// `overlay_rects` for each visible tile — matching the emission predicate
/// used by `terminal::place_rects_in_region` (clip + row-fits checks).
pub(super) fn collect_new_slots(
    visible: &VisibleTiles,
    loaded: &DisplayState,
    layout: &Layout,
    include_overlays: bool,
) -> HashSet<PlacementSlot> {
    use super::terminal::overlay_slots_for_tile;
    let mut slots = HashSet::new();
    match visible {
        VisibleTiles::Single { idx, src_y, src_h } => {
            slots.insert(PlacementSlot::Content(*idx));
            slots.insert(PlacementSlot::Sidebar(*idx));
            if include_overlays {
                overlay_slots_for_tile(
                    &mut slots,
                    *idx,
                    loaded.overlay_rects(*idx),
                    *src_y,
                    *src_h,
                    0,
                    layout.image_rows,
                    layout.cell_h as u32,
                );
            }
        }
        VisibleTiles::Split {
            top_idx,
            top_src_y,
            top_src_h,
            bot_idx,
            bot_src_h,
        } => {
            slots.insert(PlacementSlot::Content(*top_idx));
            slots.insert(PlacementSlot::Sidebar(*top_idx));
            slots.insert(PlacementSlot::Content(*bot_idx));
            slots.insert(PlacementSlot::Sidebar(*bot_idx));
            if include_overlays {
                let (top_rows, bot_rows) =
                    super::terminal::split_rows_pub(*top_src_h, layout.cell_h, layout.image_rows);
                overlay_slots_for_tile(
                    &mut slots,
                    *top_idx,
                    loaded.overlay_rects(*top_idx),
                    *top_src_y,
                    *top_src_h,
                    0,
                    top_rows,
                    layout.cell_h as u32,
                );
                overlay_slots_for_tile(
                    &mut slots,
                    *bot_idx,
                    loaded.overlay_rects(*bot_idx),
                    0,
                    *bot_src_h,
                    top_rows,
                    bot_rows,
                    layout.cell_h as u32,
                );
            }
        }
    }
    slots
}

/// Execute the I/O for a load action: send images to the terminal and evict distant tiles.
fn execute_load(action: &LoadAction, pngs: &crate::frame::TilePngs) -> anyhow::Result<()> {
    terminal::send_image(&pngs.content, action.content_id)?;
    terminal::send_image(&pngs.sidebar, action.sidebar_id)?;
    for (_, ids) in &action.evict {
        let _ = terminal::delete_image(ids.content_id);
        let _ = terminal::delete_image(ids.sidebar_id);
    }
    Ok(())
}

/// Handle for sending/receiving tile render requests directly via fork IPC.
pub(super) struct ForkHandle<'a> {
    pub renderer: &'a mut TileRenderer,
    pub in_flight: &'a mut HashSet<usize>,
}

/// Compute the visible tiles for rendering, snapping `y` to a cell boundary
/// only when the raw position would produce a `Split`. Sub-cell `y` on a
/// `Single` placement is harmless (Kitty renders `h` at native resolution
/// when `r * cell_h == h`, which always holds because `vp_h` is cell-aligned).
/// In `Split`, `top_src_h = tile_h - src_y_in_tile` is not a cell multiple if
/// `src_y_in_tile` is sub-cell, which forces Kitty to vertically compress the
/// top image; we avoid that by snapping at tile boundaries only.
fn visible_tiles_for_render(
    meta: &DocumentMeta,
    scroll: &ScrollState,
    layout: &Layout,
) -> VisibleTiles {
    let y = scroll.y_offset;
    let visible = meta.visible_tiles(y, scroll.vp_h);
    match &visible {
        VisibleTiles::Split { .. } => {
            let cell_h = layout.cell_h as u32;
            let snapped = (y / cell_h) * cell_h;
            if snapped == y {
                visible
            } else {
                meta.visible_tiles(snapped, scroll.vp_h)
            }
        }
        VisibleTiles::Single { .. } => visible,
    }
}

/// Full redraw: content tiles + sidebar + overlay + status bar.
///
/// Ordering:
///   Phase 1 — ensure visible tiles are rendered and uploaded.
///   Phase 2 — delete only the placement slots that are going away this
///             frame (same slots that stay are updated in-place by Phase 3).
///   Phase 3 — emit `a=p` with stable `(i, p)`; Kitty performs an atomic
///             in-place move when the pair already exists.
#[allow(clippy::too_many_arguments)]
pub(super) fn redraw(
    meta: &DocumentMeta,
    cache: &mut TileCache,
    loaded: &mut DisplayState,
    layout: &Layout,
    scroll: &ScrollState,
    filename: &str,
    acc_peek: Option<u32>,
    flash: Option<&str>,
    include_overlays: bool,
    rh: &mut ForkHandle<'_>,
) -> anyhow::Result<()> {
    let visible = visible_tiles_for_render(meta, scroll, layout);

    // Phase 1: Ensure all needed tiles are rendered and sent to the terminal.
    match &visible {
        VisibleTiles::Single { idx, .. } => {
            loaded.ensure_loaded(cache, *idx, rh)?;
        }
        VisibleTiles::Split {
            top_idx, bot_idx, ..
        } => {
            loaded.ensure_loaded(cache, *top_idx, rh)?;
            loaded.ensure_loaded(cache, *bot_idx, rh)?;
        }
    }

    // Phase 2: Delete only slots that disappear this frame.
    let new_slots = collect_new_slots(&visible, loaded, layout, include_overlays);
    {
        let mut out = std::io::stdout();
        loaded.delete_stale_slots(&mut out, &new_slots)?;
        out.flush()?;
    }

    // Phase 3: Place content + sidebar + overlay + status bar.
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
    rh: &mut ForkHandle<'_>,
    meta: &DocumentMeta,
    cache: &TileCache,
    y_offset: u32,
) {
    let current = (y_offset / meta.tile_height_px) as usize;
    // Forward 2 + backward 1
    for idx in [current + 1, current + 2, current.wrapping_sub(1)] {
        if idx < meta.tile_count && !cache.contains(idx) && !rh.in_flight.contains(&idx) {
            debug!("prefetch: requesting tile {idx} (current={current})");
            let _ = rh.renderer.send_render_tile(idx);
            rh.in_flight.insert(idx);
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
    loaded: &mut DisplayState,
    cache: &mut TileCache,
    layout: &Layout,
    scroll: &ScrollState,
    spec: &HighlightSpec,
    rh: &mut ForkHandle<'_>,
) -> anyhow::Result<()> {
    let visible = visible_tiles_for_render(meta, scroll, layout);

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
        let _ = rh.renderer.send_find_rects(idx, spec);
        needed += 1;
    }

    while needed > 0 {
        match rh.renderer.recv()? {
            TileResponse::Rects { idx, rects } => {
                needed -= 1;
                loaded.set_overlay_rects(idx, rects);
            }
            TileResponse::Tile { idx, pngs } => {
                rh.in_flight.remove(&idx);
                cache.insert(idx, pngs);
            }
        }
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

/// Drain all pending tile/rect responses from the child process.
///
/// Moves completed tile PNGs into the cache and overlay rects into display state.
/// Returns immediately when no data is ready (non-blocking).
pub(super) fn drain_responses(
    rh: &mut ForkHandle<'_>,
    cache: &mut TileCache,
    display: &mut DisplayState,
) -> anyhow::Result<()> {
    while let Some(resp) = rh.renderer.try_recv()? {
        match resp {
            TileResponse::Tile { idx, pngs } => {
                debug!(
                    "drain: received tile {idx} ({} + {} bytes)",
                    pngs.content.len(),
                    pngs.sidebar.len()
                );
                rh.in_flight.remove(&idx);
                cache.insert(idx, pngs);
            }
            TileResponse::Rects { idx, rects } => {
                display.set_overlay_rects(idx, rects);
            }
        }
    }
    Ok(())
}

/// Full redraw cycle: drain pending responses, render visible tiles,
/// update search overlays, and prefetch adjacent tiles.
#[allow(clippy::too_many_arguments)]
pub(super) fn redraw_and_prefetch(
    meta: &DocumentMeta,
    cache: &mut TileCache,
    display: &mut DisplayState,
    layout: &Layout,
    scroll: &ScrollState,
    filename: &str,
    acc_peek: Option<u32>,
    flash: Option<&str>,
    search_spec: Option<&HighlightSpec>,
    rh: &mut ForkHandle<'_>,
) -> anyhow::Result<()> {
    drain_responses(rh, cache, display)?;
    redraw(
        meta,
        cache,
        display,
        layout,
        scroll,
        filename,
        acc_peek,
        flash,
        search_spec.is_some(),
        rh,
    )?;
    if let Some(spec) = search_spec {
        update_overlays(meta, display, cache, layout, scroll, spec, rh)?;
        let visible = visible_tiles_for_render(meta, scroll, layout);
        // update_overlays may have populated new rects; re-emit overlay
        // placements. Stale slots (rects that vanished for the current
        // tile set) were already pruned by Phase 2's delete_stale_slots.
        terminal::place_overlay_rects(&visible, display, layout)?;
    }
    send_prefetch(rh, meta, cache, scroll.y_offset);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_load_allocates_ids() {
        let mut loaded = DisplayState::new(3);
        let action = loaded.plan_load(0).unwrap();
        assert_eq!(action.idx, 0);
        assert_eq!(action.content_id, 100);
        assert_eq!(action.sidebar_id, 101);
        assert!(action.evict.is_empty());
    }

    #[test]
    fn plan_load_already_loaded_returns_none() {
        let mut loaded = DisplayState::new(3);
        loaded.plan_load(0); // load tile 0
        assert!(loaded.plan_load(0).is_none());
    }

    #[test]
    fn plan_load_evicts_distant_tiles() {
        let mut loaded = DisplayState::new(2); // evict_distance = 2
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
        let mut loaded = DisplayState::new(3);
        let a1 = loaded.plan_load(0).unwrap();
        let a2 = loaded.plan_load(1).unwrap();
        assert_eq!(a1.content_id, 100);
        assert_eq!(a1.sidebar_id, 101);
        assert_eq!(a2.content_id, 102);
        assert_eq!(a2.sidebar_id, 103);
    }

    #[test]
    fn clear_overlay_state_empties_rects() {
        let mut loaded = DisplayState::new(3);
        loaded.set_overlay_rects(0, vec![]);
        assert!(loaded.has_overlay(0));
        loaded.clear_overlay_state();
        assert!(!loaded.has_overlay(0));
    }

    #[test]
    fn placement_id_is_stable_by_slot() {
        assert_eq!(PlacementSlot::Content(0).placement_id(), 1);
        assert_eq!(PlacementSlot::Content(42).placement_id(), 1);
        assert_eq!(PlacementSlot::Sidebar(0).placement_id(), 1);
        assert_eq!(PlacementSlot::OverlayPrimary(5, 0).placement_id(), 1);
        assert_eq!(PlacementSlot::OverlayOverflow(5, 0).placement_id(), 2);
        assert_eq!(PlacementSlot::OverlayPrimary(5, 3).placement_id(), 7);
        assert_eq!(PlacementSlot::OverlayOverflow(5, 3).placement_id(), 8);
    }

    #[test]
    fn track_placement_emits_no_delete_for_new_slot() {
        let mut loaded = DisplayState::new(3);
        let mut out = Vec::<u8>::new();
        let pid = loaded
            .track_placement(&mut out, PlacementSlot::Content(0), 100)
            .unwrap();
        assert_eq!(pid, 1);
        // No prior entry → no stale delete emitted.
        assert!(out.is_empty());
        assert_eq!(loaded.live_slot_image(PlacementSlot::Content(0)), Some(100));
    }

    #[test]
    fn track_placement_emits_delete_when_image_changes() {
        let mut loaded = DisplayState::new(3);
        let mut out = Vec::<u8>::new();
        // Simulate overlay flipping from inactive (image 200) to active (image 204).
        let slot = PlacementSlot::OverlayPrimary(3, 0);
        loaded.track_placement(&mut out, slot, 200).unwrap();
        out.clear();
        loaded.track_placement(&mut out, slot, 204).unwrap();
        // Old (i=200, p=1) must be deleted so it does not linger.
        let emitted = String::from_utf8(out).unwrap();
        assert!(
            emitted.contains("a=d,d=i,i=200,p=1"),
            "expected stale delete for old image_id; got {emitted:?}"
        );
        assert_eq!(loaded.live_slot_image(slot), Some(204));
    }

    #[test]
    fn track_placement_same_image_is_noop() {
        let mut loaded = DisplayState::new(3);
        let mut out = Vec::<u8>::new();
        let slot = PlacementSlot::Content(0);
        loaded.track_placement(&mut out, slot, 100).unwrap();
        out.clear();
        loaded.track_placement(&mut out, slot, 100).unwrap();
        // Same (i, p) is atomic in-place — no extra delete should be emitted.
        assert!(out.is_empty());
    }

    #[test]
    fn delete_stale_slots_removes_only_absent_slots() {
        let mut loaded = DisplayState::new(3);
        let mut out = Vec::<u8>::new();
        loaded
            .track_placement(&mut out, PlacementSlot::Content(0), 100)
            .unwrap();
        loaded
            .track_placement(&mut out, PlacementSlot::Content(1), 102)
            .unwrap();
        loaded
            .track_placement(&mut out, PlacementSlot::Sidebar(0), 101)
            .unwrap();
        out.clear();

        let mut keep = HashSet::new();
        keep.insert(PlacementSlot::Content(0));
        keep.insert(PlacementSlot::Sidebar(0));
        loaded.delete_stale_slots(&mut out, &keep).unwrap();

        let emitted = String::from_utf8(out).unwrap();
        // Only Content(1) is stale → only image_id 102 gets deleted.
        assert!(emitted.contains("i=102,p=1"), "got {emitted:?}");
        assert!(!emitted.contains("i=100"));
        assert!(!emitted.contains("i=101"));
        assert_eq!(loaded.live_slot_count(), 2);
    }

    #[test]
    fn eviction_drops_live_slots_for_evicted_tile() {
        let mut loaded = DisplayState::new(2); // evict_distance = 2
        loaded.plan_load(0);
        let mut out = Vec::<u8>::new();
        // Track a content placement for tile 0.
        loaded
            .track_placement(&mut out, PlacementSlot::Content(0), 100)
            .unwrap();
        // Load tile 5 — distance from 0 is 5 > 2, so tile 0 is evicted.
        loaded.plan_load(5);
        assert!(
            loaded.live_slot_image(PlacementSlot::Content(0)).is_none(),
            "evicted tile's live slots must be cleared"
        );
    }

    #[test]
    fn delete_overlay_placements_clears_overlay_slots_only() {
        let mut loaded = DisplayState::new(3);
        let mut out = Vec::<u8>::new();
        loaded
            .track_placement(&mut out, PlacementSlot::Content(0), 100)
            .unwrap();
        loaded
            .track_placement(&mut out, PlacementSlot::OverlayPrimary(0, 0), 200)
            .unwrap();
        // delete_overlay_placements needs highlight_images uploaded for I/O;
        // without them it's a no-op on stdout — but still clears slot tracker.
        loaded.delete_overlay_placements().unwrap();
        assert_eq!(
            loaded.live_slot_image(PlacementSlot::Content(0)),
            Some(100),
            "tile slots must survive overlay deletion"
        );
        assert!(
            loaded
                .live_slot_image(PlacementSlot::OverlayPrimary(0, 0))
                .is_none(),
            "overlay slots must be cleared"
        );
    }

    #[test]
    fn delete_placements_clears_all_live_slots() {
        let mut loaded = DisplayState::new(3);
        let mut out = Vec::<u8>::new();
        loaded
            .track_placement(&mut out, PlacementSlot::Content(0), 100)
            .unwrap();
        loaded
            .track_placement(&mut out, PlacementSlot::OverlayPrimary(0, 0), 200)
            .unwrap();
        loaded.delete_placements().unwrap();
        assert_eq!(loaded.live_slot_count(), 0);
    }

    #[test]
    fn clear_all_resets_live_slots() {
        let mut loaded = DisplayState::new(3);
        let mut out = Vec::<u8>::new();
        loaded
            .track_placement(&mut out, PlacementSlot::Content(0), 100)
            .unwrap();
        loaded.clear_all();
        assert_eq!(loaded.live_slot_count(), 0);
    }
}
