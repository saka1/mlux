//! KGP placement emission: tile content, sidebar, and highlight overlays.
//!
//! Uses `PlacementLedger` to diff against last-emitted state and emit only
//! changed placements. Unchanged placements are skipped entirely; disappeared
//! slots get a targeted `a=d` delete instead of the old blanket delete-all.

use crossterm::{QueueableCommand, cursor};
use log::debug;
use std::collections::HashSet;
use std::io::{self, Write, stdout};

use super::super::layout::{Layout, ScrollState};
use super::ledger::{DiffOp, PlacementSlot, PlacementSpec};
use super::{HighlightImages, TilePresenter};
use crate::frame::VisibleTiles;

/// Compute screen row counts for a split tile pair.
fn split_rows(top_src_h: u32, cell_h: u16, image_rows: u16) -> (u16, u16) {
    let top = (top_src_h as f64 / cell_h as f64).round() as u16;
    let top = top.clamp(1, image_rows.saturating_sub(1));
    let bot = image_rows.saturating_sub(top);
    (top, bot)
}

/// Emit a `a=p` Kitty placement command and record it in the ledger.
///
/// If the spec matches the ledger entry, skip. Otherwise emit and update.
fn diff_and_emit(
    out: &mut impl Write,
    loaded: &mut TilePresenter,
    slot: PlacementSlot,
    spec: PlacementSpec,
) -> io::Result<()> {
    if loaded.ledger.diff(slot, &spec) == DiffOp::Skip {
        return Ok(());
    }
    let PlacementSpec {
        image_id: i,
        placement_id: p,
        screen_col,
        screen_row,
        cols,
        rows,
        src_x: x,
        src_y: y,
        src_w: w,
        src_h: h,
        x_off: x_off_px,
        y_off: y_off_px,
        z,
    } = spec;
    out.queue(cursor::MoveTo(screen_col, screen_row))?;
    if z == 0 {
        // Tile placement: source-rect params, no sub-cell offsets
        write!(
            out,
            "\x1b_Ga=p,i={i},p={p},x={x},y={y},w={w},h={h},c={cols},r={rows},C=1,q=2\x1b\\",
        )?;
    } else {
        // Overlay placement with sub-cell offsets (KGP X= Y= params)
        write!(
            out,
            "\x1b_Ga=p,i={i},p={p},w={w},h={h},X={x_off_px},Y={y_off_px},c={cols},r={rows},C=1,z={z},q=2\x1b\\",
        )?;
    }
    loaded.ledger.record(slot, spec);
    Ok(())
}

/// Delete stray placements whose slots are no longer in `keep`.
pub(super) fn delete_stray_placements(
    out: &mut impl Write,
    loaded: &mut TilePresenter,
    keep: &HashSet<PlacementSlot>,
) -> io::Result<()> {
    let stray = loaded.ledger.retain_slots(keep);
    for (_, spec) in stray {
        write!(
            out,
            "\x1b_Ga=d,d=i,i={},p={},q=2\x1b\\",
            spec.image_id, spec.placement_id,
        )?;
    }
    Ok(())
}

/// Place content tile(s) based on visible_tiles result.
pub(super) fn place_content_tiles(
    visible: &VisibleTiles,
    loaded: &mut TilePresenter,
    layout: &Layout,
    scroll: &ScrollState,
) -> io::Result<()> {
    let mut out = stdout();
    let w = scroll.vp_w;
    let cols = layout.image_cols;
    let start_col = layout.image_col;

    match visible {
        VisibleTiles::Single { idx, src_y, src_h } => {
            let ids = loaded.map.get(idx).unwrap();
            let image_id = ids.content_id;
            let rows = ((*src_h as f64) / (layout.cell_h as f64))
                .ceil()
                .min(layout.image_rows as f64) as u16;
            let rows = rows.max(1);
            let spec = PlacementSpec {
                image_id,
                placement_id: 1,
                screen_col: start_col,
                screen_row: 0,
                cols,
                rows,
                src_x: 0,
                src_y: *src_y,
                src_w: w,
                src_h: *src_h,
                x_off: 0,
                y_off: 0,
                z: 0,
            };
            diff_and_emit(&mut out, loaded, PlacementSlot::Content(*idx), spec)?;
        }
        VisibleTiles::Split {
            top_idx,
            top_src_y,
            top_src_h,
            bot_idx,
            bot_src_h,
        } => {
            let (top_rows, bot_rows) = split_rows(*top_src_h, layout.cell_h, layout.image_rows);

            let top_display = top_rows as u32 * layout.cell_h as u32;
            let bot_display = bot_rows as u32 * layout.cell_h as u32;
            debug!(
                "place_content split: top tile={top_idx} src_y={top_src_y} src_h={top_src_h} -> {top_rows}r ({top_display}px, {:.3}x), \
                 bot tile={bot_idx} src_h={bot_src_h} -> {bot_rows}r ({bot_display}px, {:.3}x)",
                top_display as f64 / *top_src_h as f64,
                bot_display as f64 / *bot_src_h as f64,
            );

            let top_image_id = loaded.map.get(top_idx).unwrap().content_id;
            let bot_image_id = loaded.map.get(bot_idx).unwrap().content_id;

            diff_and_emit(
                &mut out,
                loaded,
                PlacementSlot::Content(*top_idx),
                PlacementSpec {
                    image_id: top_image_id,
                    placement_id: 1,
                    screen_col: start_col,
                    screen_row: 0,
                    cols,
                    rows: top_rows,
                    src_x: 0,
                    src_y: *top_src_y,
                    src_w: w,
                    src_h: *top_src_h,
                    x_off: 0,
                    y_off: 0,
                    z: 0,
                },
            )?;
            diff_and_emit(
                &mut out,
                loaded,
                PlacementSlot::Content(*bot_idx),
                PlacementSpec {
                    image_id: bot_image_id,
                    placement_id: 1,
                    screen_col: start_col,
                    screen_row: top_rows,
                    cols,
                    rows: bot_rows,
                    src_x: 0,
                    src_y: 0,
                    src_w: w,
                    src_h: *bot_src_h,
                    x_off: 0,
                    y_off: 0,
                    z: 0,
                },
            )?;
        }
    }
    out.flush()
}

/// Place sidebar tile(s) based on the same visible_tiles as content.
pub(super) fn place_sidebar_tiles(
    visible: &VisibleTiles,
    loaded: &mut TilePresenter,
    sidebar_width_px: u32,
    layout: &Layout,
) -> io::Result<()> {
    let mut out = stdout();
    let w = sidebar_width_px;
    let cols = layout.sidebar_cols;

    match visible {
        VisibleTiles::Single { idx, src_y, src_h } => {
            let ids = loaded.map.get(idx).unwrap();
            let image_id = ids.sidebar_id;
            let rows = ((*src_h as f64) / (layout.cell_h as f64))
                .ceil()
                .min(layout.image_rows as f64) as u16;
            let rows = rows.max(1);
            diff_and_emit(
                &mut out,
                loaded,
                PlacementSlot::Sidebar(*idx),
                PlacementSpec {
                    image_id,
                    placement_id: 1,
                    screen_col: 0,
                    screen_row: 0,
                    cols,
                    rows,
                    src_x: 0,
                    src_y: *src_y,
                    src_w: w,
                    src_h: *src_h,
                    x_off: 0,
                    y_off: 0,
                    z: 0,
                },
            )?;
        }
        VisibleTiles::Split {
            top_idx,
            top_src_y,
            top_src_h,
            bot_idx,
            bot_src_h,
        } => {
            let (top_rows, bot_rows) = split_rows(*top_src_h, layout.cell_h, layout.image_rows);
            let top_image_id = loaded.map.get(top_idx).unwrap().sidebar_id;
            let bot_image_id = loaded.map.get(bot_idx).unwrap().sidebar_id;

            diff_and_emit(
                &mut out,
                loaded,
                PlacementSlot::Sidebar(*top_idx),
                PlacementSpec {
                    image_id: top_image_id,
                    placement_id: 1,
                    screen_col: 0,
                    screen_row: 0,
                    cols,
                    rows: top_rows,
                    src_x: 0,
                    src_y: *top_src_y,
                    src_w: w,
                    src_h: *top_src_h,
                    x_off: 0,
                    y_off: 0,
                    z: 0,
                },
            )?;
            diff_and_emit(
                &mut out,
                loaded,
                PlacementSlot::Sidebar(*bot_idx),
                PlacementSpec {
                    image_id: bot_image_id,
                    placement_id: 1,
                    screen_col: 0,
                    screen_row: top_rows,
                    cols,
                    rows: bot_rows,
                    src_x: 0,
                    src_y: 0,
                    src_w: w,
                    src_h: *bot_src_h,
                    x_off: 0,
                    y_off: 0,
                    z: 0,
                },
            )?;
        }
    }
    out.flush()
}

/// Compute the set of PlacementSlots for currently visible tiles.
/// Used by the caller to delete stray placements before/after emit.
pub(super) fn visible_tile_slots(visible: &VisibleTiles) -> HashSet<PlacementSlot> {
    let mut slots = HashSet::new();
    match visible {
        VisibleTiles::Single { idx, .. } => {
            slots.insert(PlacementSlot::Content(*idx));
            slots.insert(PlacementSlot::Sidebar(*idx));
        }
        VisibleTiles::Split {
            top_idx, bot_idx, ..
        } => {
            slots.insert(PlacementSlot::Content(*top_idx));
            slots.insert(PlacementSlot::Sidebar(*top_idx));
            slots.insert(PlacementSlot::Content(*bot_idx));
            slots.insert(PlacementSlot::Sidebar(*bot_idx));
        }
    }
    slots
}

/// Place highlight rectangles using Y sub-cell offset for pixel-precise
/// vertical alignment, with partial-transparency overflow patterns.
pub(super) fn place_overlay_rects(
    visible: &VisibleTiles,
    loaded: &mut TilePresenter,
    layout: &Layout,
) -> io::Result<()> {
    // Clone to avoid holding &self borrow while diff_and_emit needs &mut self below.
    let imgs = match loaded.highlight_images().cloned() {
        Some(imgs) => imgs,
        None => return Ok(()),
    };

    let cw = layout.cell_w as u32;
    let ch = layout.cell_h as u32;
    if cw == 0 || ch == 0 {
        return Ok(());
    }

    let mut out = stdout();

    match visible {
        VisibleTiles::Single { idx, src_y, src_h } => {
            place_rects_in_region(
                &mut out,
                loaded,
                *idx,
                imgs,
                &TileRegion {
                    src_y: *src_y,
                    src_h: *src_h,
                    screen_row: 0,
                    max_rows: layout.image_rows,
                    image_col: layout.image_col,
                    cw,
                    ch,
                },
            )?;
        }
        VisibleTiles::Split {
            top_idx,
            top_src_y,
            top_src_h,
            bot_idx,
            bot_src_h,
        } => {
            let (top_rows, bot_rows) = split_rows(*top_src_h, layout.cell_h, layout.image_rows);

            place_rects_in_region(
                &mut out,
                loaded,
                *top_idx,
                imgs.clone(),
                &TileRegion {
                    src_y: *top_src_y,
                    src_h: *top_src_h,
                    screen_row: 0,
                    max_rows: top_rows,
                    image_col: layout.image_col,
                    cw,
                    ch,
                },
            )?;
            place_rects_in_region(
                &mut out,
                loaded,
                *bot_idx,
                imgs,
                &TileRegion {
                    src_y: 0,
                    src_h: *bot_src_h,
                    screen_row: top_rows,
                    max_rows: bot_rows,
                    image_col: layout.image_col,
                    cw,
                    ch,
                },
            )?;
        }
    }
    out.flush()
}

/// Describes a visible region of a tile mapped to screen rows.
struct TileRegion {
    src_y: u32,
    src_h: u32,
    screen_row: u16,
    max_rows: u16,
    image_col: u16,
    cw: u32,
    ch: u32,
}

/// Place highlight rects for a single tile region.
fn place_rects_in_region(
    out: &mut impl Write,
    loaded: &mut TilePresenter,
    tile_idx: usize,
    imgs: HighlightImages,
    rgn: &TileRegion,
) -> io::Result<()> {
    use crate::frame::{
        HIGHLIGHT_PNG_HEIGHT, HIGHLIGHT_PNG_WIDTH, PATTERN_HEIGHT, PATTERN_WIDTH, PartialPattern,
        select_overflow_pattern,
    };

    // Clone rects to avoid borrow conflict with `loaded` (ledger mutation below)
    let rects: Vec<_> = loaded.overlay_rects(tile_idx).to_vec();

    for (rect_idx, r) in rects.iter().enumerate() {
        // Clip to visible region of tile
        if r.y_px + r.h_px <= rgn.src_y || r.y_px >= rgn.src_y + rgn.src_h {
            continue;
        }

        // Select image IDs based on active state
        let full_id = if r.is_active {
            imgs.active_full_id
        } else {
            imgs.full_id
        };
        let p75_id = if r.is_active {
            imgs.active_p75_id
        } else {
            imgs.p75_id
        };
        let p50_id = if r.is_active {
            imgs.active_p50_id
        } else {
            imgs.p50_id
        };
        let p25_id = if r.is_active {
            imgs.active_p25_id
        } else {
            imgs.p25_id
        };

        // Row + Y offset from top edge (not midpoint)
        let screen_y_px = r.y_px.saturating_sub(rgn.src_y);
        let row = rgn.screen_row + (screen_y_px / rgn.ch) as u16;
        let y_off = screen_y_px % rgn.ch;
        let col = rgn.image_col + (r.x_px / rgn.cw) as u16;

        // Horizontal: sub-cell X offset for pixel-precise start.
        let x_off = r.x_px % rgn.cw;
        let cols = (r.w_px + x_off).div_ceil(rgn.cw).max(1) as u16;

        // Source-rect: crop to exact highlight width; use full image height.
        let src_w = r.w_px.min(HIGHLIGHT_PNG_WIDTH);
        let src_h = HIGHLIGHT_PNG_HEIGHT;

        // Clamp to available screen space
        if row >= rgn.screen_row + rgn.max_rows {
            continue;
        }

        debug!(
            "hl: col={col} row={row} X={x_off} Y={y_off} c={cols} \
             src={}x{} want={}x{} active={}",
            src_w, src_h, r.w_px, r.h_px, r.is_active,
        );

        // Primary placement ID: 2*rect_idx + 1
        let pid_primary = (2 * rect_idx + 1) as u32;
        let primary_spec = PlacementSpec {
            image_id: full_id,
            placement_id: pid_primary,
            screen_col: col,
            screen_row: row,
            cols,
            rows: 1,
            src_x: 0,
            src_y: 0,
            src_w,
            src_h,
            x_off,
            y_off,
            z: 1,
        };
        diff_and_emit(
            out,
            loaded,
            PlacementSlot::OverlayPrimary(tile_idx, rect_idx),
            primary_spec,
        )?;

        // Overflow placement: 2*rect_idx + 2
        let first_coverage = rgn.ch - y_off;
        if r.h_px > first_coverage {
            let next_row = row + 1;
            if next_row < rgn.screen_row + rgn.max_rows {
                let overflow = (r.h_px - first_coverage).min(rgn.ch);
                let pattern = select_overflow_pattern(overflow, rgn.ch);

                let (ov_id, ov_w, ov_h) = match pattern {
                    PartialPattern::Full => (full_id, src_w, HIGHLIGHT_PNG_HEIGHT),
                    PartialPattern::P75 => (p75_id, PATTERN_WIDTH, PATTERN_HEIGHT),
                    PartialPattern::P50 => (p50_id, PATTERN_WIDTH, PATTERN_HEIGHT),
                    PartialPattern::P25 => (p25_id, PATTERN_WIDTH, PATTERN_HEIGHT),
                };

                debug!(
                    "hl overflow: next_row={next_row} overflow={overflow}px pattern={pattern:?}",
                );

                let pid_overflow = (2 * rect_idx + 2) as u32;
                let overflow_spec = PlacementSpec {
                    image_id: ov_id,
                    placement_id: pid_overflow,
                    screen_col: col,
                    screen_row: next_row,
                    cols,
                    rows: 1,
                    src_x: 0,
                    src_y: 0,
                    src_w: ov_w,
                    src_h: ov_h,
                    x_off,
                    y_off: 0,
                    z: 1,
                };
                diff_and_emit(
                    out,
                    loaded,
                    PlacementSlot::OverlayOverflow(tile_idx, rect_idx),
                    overflow_spec,
                )?;
            }
        }
    }
    Ok(())
}
