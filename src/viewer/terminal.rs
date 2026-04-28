//! Terminal I/O layer: raw mode, Kitty Graphics Protocol, status bar, OSC 52.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    ExecutableCommand, QueueableCommand, cursor,
    event::{DisableMouseCapture, EnableMouseCapture},
    style::{self, Stylize},
    terminal,
};
use log::{debug, warn};
use std::io::{self, Write, stdout};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

use super::display_state::{DisplayState, PlacementSlot};
use super::layout::{Layout, ScrollState};
use crate::frame::VisibleTiles;

// ---------------------------------------------------------------------------
// RawGuard — restores raw mode / alternate screen / image cleanup on Drop
// ---------------------------------------------------------------------------

pub(super) struct RawGuard {
    cleaned: bool,
    mouse: bool,
}

impl RawGuard {
    pub(super) fn enter(mouse: bool) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        stdout().execute(terminal::EnterAlternateScreen)?;
        if mouse {
            stdout().execute(EnableMouseCapture)?;
        }
        stdout().execute(cursor::Hide)?;
        Ok(Self {
            cleaned: false,
            mouse,
        })
    }

    pub(super) fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        let mut out = stdout();
        let _ = write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\");
        let _ = out.execute(cursor::Show);
        if self.mouse {
            let _ = out.execute(DisableMouseCapture);
        }
        let _ = out.execute(terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// ---------------------------------------------------------------------------
// Kitty protocol helpers
// ---------------------------------------------------------------------------

/// Send PNG data via temp file (a=t: transfer only, no display).
///
/// Writes `png_data` to a temp file, then sends a single Kitty escape
/// sequence with `t=t` (temp file transfer). Kitty reads and deletes
/// the file, avoiding base64-encoding the full payload through the pty.
pub(super) fn send_image(png_data: &[u8], image_id: u32) -> io::Result<()> {
    let start = Instant::now();

    let mut tmp = tempfile::Builder::new()
        .prefix("tty-graphics-protocol.")
        .tempfile()?;
    tmp.write_all(png_data)?;
    tmp.flush()?;

    // Get path, then close handle and disarm auto-delete.
    // Kitty deletes the file after reading (t=t).
    let path = tmp
        .path()
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "temp path not UTF-8"))?
        .to_string();
    let _ = tmp.into_temp_path().keep();

    let encoded_path = BASE64.encode(path.as_bytes());
    let mut out = stdout();
    write!(
        out,
        "\x1b_Ga=t,f=100,i={image_id},t=t,q=2;{encoded_path}\x1b\\"
    )?;
    out.flush()?;

    debug!(
        "kgp: send_image id={image_id} ({} bytes) tmpfile {:.1}ms [{}]",
        png_data.len(),
        start.elapsed().as_secs_f64() * 1000.0,
        path,
    );
    Ok(())
}

/// Send raw RGBA data (a=t: transfer only, no display).
///
/// Small payloads (e.g. 96 bytes for 1×24 RGBA) fit in a single chunk.
pub(super) fn send_raw_image(
    rgba: &[u8],
    width: u32,
    height: u32,
    image_id: u32,
) -> io::Result<()> {
    let encoded = BASE64.encode(rgba);
    let mut out = stdout();
    write!(
        out,
        "\x1b_Ga=t,f=32,s={width},v={height},i={image_id},t=d,q=2;{encoded}\x1b\\"
    )?;
    out.flush()
}

/// Delete image data and placements
pub(super) fn delete_image(image_id: u32) -> io::Result<()> {
    let mut out = stdout();
    write!(out, "\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")?;
    out.flush()
}

/// Delete all images and data.
///
/// INVARIANT: any caller must ensure the corresponding `DisplayState.live_slots`
/// is cleared — either via [`DisplayState::clear_all`] or by dropping the
/// `DisplayState` entirely — before the next frame emits placements. Otherwise
/// `live_slots` would claim slots the terminal no longer has (phantom state),
/// causing spurious `a=d` on the next redraw.
///
/// Current callers:
/// - `RenderOp::DeleteAllImages` (effect.rs) — paired with `clear_all()`
/// - `Session::update_layout_for_resize` / `handle_exit::Navigate|GoBack` —
///   paired by the outer `'outer` loop in `viewer::run` which constructs a
///   fresh `DisplayState` for the next iteration.
pub(super) fn delete_all_images() -> io::Result<()> {
    let mut out = stdout();
    write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\")?;
    out.flush()
}

/// Delete image data and placements for specific IDs (old-generation cleanup)
pub(super) fn delete_images_by_ids(ids: &[u32]) -> io::Result<()> {
    debug!("kgp: delete_images_by_ids ({} images)", ids.len());
    let mut out = stdout();
    for &id in ids {
        write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
    }
    out.flush()
}

/// Clear the text layer (wipe search/command screen text)
pub(super) fn clear_screen() -> io::Result<()> {
    let mut out = stdout();
    out.queue(terminal::Clear(terminal::ClearType::All))?;
    out.flush()
}

/// Parameters for placing tile images via Kitty Graphics Protocol.
pub(super) struct PlaceParams {
    pub start_col: u16,
    pub num_cols: u16,
    pub img_width: u32,
}

/// Compute screen row counts for a split tile pair.
fn split_rows(top_src_h: u32, cell_h: u16, image_rows: u16) -> (u16, u16) {
    let top = (top_src_h as f64 / cell_h as f64).round() as u16;
    let top = top.clamp(1, image_rows.saturating_sub(1));
    let bot = image_rows.saturating_sub(top);
    (top, bot)
}

/// Exposed alias of `split_rows` for `display_state::collect_new_slots`.
pub(super) fn split_rows_pub(top_src_h: u32, cell_h: u16, image_rows: u16) -> (u16, u16) {
    split_rows(top_src_h, cell_h, image_rows)
}

/// Place tile(s) using Kitty Graphics Protocol.
///
/// `get_id` selects which image ID to use from a `TileImageIds`;
/// `make_slot(tile_idx)` builds the corresponding `PlacementSlot` so that
/// the emitted `a=p` carries a stable `p=<placement_id>` and the live-slot
/// tracker can handle future diffing.
pub(super) fn place_tiles(
    visible: &VisibleTiles,
    loaded: &mut DisplayState,
    layout: &Layout,
    params: &PlaceParams,
    get_id: fn(&super::display_state::TileImageIds) -> u32,
    make_slot: fn(usize) -> PlacementSlot,
) -> io::Result<()> {
    let mut out = stdout();
    let w = params.img_width;
    let cols = params.num_cols;

    match visible {
        VisibleTiles::Single { idx, src_y, src_h } => {
            let id = get_id(loaded.map.get(idx).unwrap());
            let rows = ((*src_h as f64) / (layout.cell_h as f64))
                .ceil()
                .min(layout.image_rows as f64) as u16;
            let rows = rows.max(1);
            let slot = make_slot(*idx);
            let pid = loaded.track_placement(&mut out, slot, id)?;
            out.queue(cursor::MoveTo(params.start_col, 0))?;
            write!(
                out,
                "\x1b_Ga=p,i={id},p={pid},x=0,y={src_y},w={w},h={src_h},c={cols},r={rows},C=1,q=2\x1b\\",
            )?;
        }
        VisibleTiles::Split {
            top_idx,
            top_src_y,
            top_src_h,
            bot_idx,
            bot_src_h,
        } => {
            let top_id = get_id(loaded.map.get(top_idx).unwrap());
            let bot_id = get_id(loaded.map.get(bot_idx).unwrap());

            let (top_rows, bot_rows) = split_rows(*top_src_h, layout.cell_h, layout.image_rows);

            let top_display = top_rows as u32 * layout.cell_h as u32;
            let bot_display = bot_rows as u32 * layout.cell_h as u32;
            debug!(
                "place_tiles split: top tile={top_idx} src_y={top_src_y} src_h={top_src_h} -> {top_rows}r ({top_display}px, {:.3}x), \
                 bot tile={bot_idx} src_h={bot_src_h} -> {bot_rows}r ({bot_display}px, {:.3}x)",
                top_display as f64 / *top_src_h as f64,
                bot_display as f64 / *bot_src_h as f64,
            );

            let top_slot = make_slot(*top_idx);
            let top_pid = loaded.track_placement(&mut out, top_slot, top_id)?;
            out.queue(cursor::MoveTo(params.start_col, 0))?;
            write!(
                out,
                "\x1b_Ga=p,i={top_id},p={top_pid},x=0,y={top_src_y},w={w},h={top_src_h},c={cols},r={top_rows},C=1,q=2\x1b\\",
            )?;
            let bot_slot = make_slot(*bot_idx);
            let bot_pid = loaded.track_placement(&mut out, bot_slot, bot_id)?;
            out.queue(cursor::MoveTo(params.start_col, top_rows))?;
            write!(
                out,
                "\x1b_Ga=p,i={bot_id},p={bot_pid},x=0,y=0,w={w},h={bot_src_h},c={cols},r={bot_rows},C=1,q=2\x1b\\",
            )?;
        }
    }
    out.flush()
}

/// Place content tile(s) based on visible_tiles result.
pub(super) fn place_content_tiles(
    visible: &VisibleTiles,
    loaded: &mut DisplayState,
    layout: &Layout,
    scroll: &ScrollState,
) -> io::Result<()> {
    place_tiles(
        visible,
        loaded,
        layout,
        &PlaceParams {
            start_col: layout.image_col,
            num_cols: layout.image_cols,
            img_width: scroll.vp_w,
        },
        |ids| ids.content_id,
        PlacementSlot::Content,
    )
}

/// Place sidebar tile(s) based on the same visible_tiles as content.
pub(super) fn place_sidebar_tiles(
    visible: &VisibleTiles,
    loaded: &mut DisplayState,
    sidebar_width_px: u32,
    layout: &Layout,
) -> io::Result<()> {
    place_tiles(
        visible,
        loaded,
        layout,
        &PlaceParams {
            start_col: 0,
            num_cols: layout.sidebar_cols,
            img_width: sidebar_width_px,
        },
        |ids| ids.sidebar_id,
        PlacementSlot::Sidebar,
    )
}

/// Place highlight rectangles using Y sub-cell offset for pixel-precise
/// vertical alignment, with partial-transparency overflow patterns.
///
/// Each rect gets a primary placement at `row = top / ch` with `Y = top % ch`,
/// and optionally a second placement on the next row for overflow coverage.
pub(super) fn place_overlay_rects(
    visible: &VisibleTiles,
    loaded: &mut DisplayState,
    layout: &Layout,
) -> io::Result<()> {
    // Snapshot image IDs so we can hold `&mut loaded` while iterating rects.
    // `HighlightImages` is 8 u32s — copying avoids a borrow conflict with
    // `loaded.track_placement` inside `place_rects_in_region`.
    let imgs = match loaded.highlight_images() {
        Some(imgs) => HighlightImagesCopy::from(imgs),
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
            let rects = loaded.overlay_rects(*idx).to_vec();
            place_rects_in_region(
                &mut out,
                loaded,
                *idx,
                &rects,
                &imgs,
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
            let top_rects = loaded.overlay_rects(*top_idx).to_vec();
            let bot_rects = loaded.overlay_rects(*bot_idx).to_vec();

            place_rects_in_region(
                &mut out,
                loaded,
                *top_idx,
                &top_rects,
                &imgs,
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
                &bot_rects,
                &imgs,
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

/// Copy of `HighlightImages` for use during rect iteration while
/// `&mut DisplayState` is held for slot tracking.
#[derive(Clone, Copy)]
struct HighlightImagesCopy {
    full_id: u32,
    p75_id: u32,
    p50_id: u32,
    p25_id: u32,
    active_full_id: u32,
    active_p75_id: u32,
    active_p50_id: u32,
    active_p25_id: u32,
}

impl From<&super::display_state::HighlightImages> for HighlightImagesCopy {
    fn from(h: &super::display_state::HighlightImages) -> Self {
        Self {
            full_id: h.full_id,
            p75_id: h.p75_id,
            p50_id: h.p50_id,
            p25_id: h.p25_id,
            active_full_id: h.active_full_id,
            active_p75_id: h.active_p75_id,
            active_p50_id: h.active_p50_id,
            active_p25_id: h.active_p25_id,
        }
    }
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

/// Shared predicate used by both slot collection (display_state) and
/// placement emission (below): decide whether a rect contributes a primary
/// slot, and whether it additionally contributes an overflow slot.
struct RectEmission {
    row: u16,
    col: u16,
    y_off: u32,
    x_off: u32,
    cols: u16,
    overflow_next_row: Option<u16>,
}

fn rect_emission(r: &crate::frame::HighlightRect, rgn: &TileRegion) -> Option<RectEmission> {
    // Clip to visible region of tile
    if r.y_px + r.h_px <= rgn.src_y || r.y_px >= rgn.src_y + rgn.src_h {
        return None;
    }

    let screen_y_px = r.y_px.saturating_sub(rgn.src_y);
    let row = rgn.screen_row + (screen_y_px / rgn.ch) as u16;
    if row >= rgn.screen_row + rgn.max_rows {
        return None;
    }
    let y_off = screen_y_px % rgn.ch;
    let col = rgn.image_col + (r.x_px / rgn.cw) as u16;
    let x_off = r.x_px % rgn.cw;
    let cols = (r.w_px + x_off).div_ceil(rgn.cw).max(1) as u16;

    let first_coverage = rgn.ch - y_off;
    let overflow_next_row = if r.h_px > first_coverage {
        let next_row = row + 1;
        if next_row < rgn.screen_row + rgn.max_rows {
            Some(next_row)
        } else {
            None
        }
    } else {
        None
    };

    Some(RectEmission {
        row,
        col,
        y_off,
        x_off,
        cols,
        overflow_next_row,
    })
}

/// Populate `slots` with overlay `PlacementSlot`s that would be emitted
/// for `rects` in the given region. Must stay in sync with the predicates
/// used by `place_rects_in_region` — `rect_emission` is the shared gate.
#[allow(clippy::too_many_arguments)]
pub(super) fn overlay_slots_for_tile(
    slots: &mut std::collections::HashSet<PlacementSlot>,
    tile_idx: usize,
    rects: &[crate::frame::HighlightRect],
    src_y: u32,
    src_h: u32,
    screen_row: u16,
    max_rows: u16,
    ch: u32,
) {
    // `cell_w` / `image_col` are irrelevant to slot identity (they affect
    // `x_off` / `col`, not the rect → slot mapping), so use dummy values
    // that still exercise the clip/row predicates faithfully.
    let rgn = TileRegion {
        src_y,
        src_h,
        screen_row,
        max_rows,
        image_col: 0,
        cw: 1,
        ch,
    };
    for (rect_idx, r) in rects.iter().enumerate() {
        if let Some(e) = rect_emission(r, &rgn) {
            slots.insert(PlacementSlot::OverlayPrimary(tile_idx, rect_idx));
            if e.overflow_next_row.is_some() {
                slots.insert(PlacementSlot::OverlayOverflow(tile_idx, rect_idx));
            }
        }
    }
}

/// Place highlight rects for a single tile region.
///
/// Uses Y sub-cell offset for pixel-precise vertical alignment. When a rect
/// overflows into the next cell row, a second placement with a partial
/// transparency pattern covers the overflow.
fn place_rects_in_region(
    out: &mut impl Write,
    loaded: &mut DisplayState,
    tile_idx: usize,
    rects: &[crate::frame::HighlightRect],
    imgs: &HighlightImagesCopy,
    rgn: &TileRegion,
) -> io::Result<()> {
    use crate::frame::{
        HIGHLIGHT_PNG_HEIGHT, HIGHLIGHT_PNG_WIDTH, PATTERN_HEIGHT, PATTERN_WIDTH, PartialPattern,
        select_overflow_pattern,
    };

    for (rect_idx, r) in rects.iter().enumerate() {
        let Some(e) = rect_emission(r, rgn) else {
            continue;
        };

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

        // Source-rect: crop to exact highlight width; use full image height.
        let src_w = r.w_px.min(HIGHLIGHT_PNG_WIDTH);
        let src_h = HIGHLIGHT_PNG_HEIGHT;

        debug!(
            "hl: col={} row={} X={} Y={} c={} src={}x{} want={}x{} active={}",
            e.col, e.row, e.x_off, e.y_off, e.cols, src_w, src_h, r.w_px, r.h_px, r.is_active,
        );

        // 1st placement: FULL image with Y sub-cell offset
        let primary_slot = PlacementSlot::OverlayPrimary(tile_idx, rect_idx);
        let primary_pid = loaded.track_placement(out, primary_slot, full_id)?;
        out.queue(cursor::MoveTo(e.col, e.row))?;
        write!(
            out,
            "\x1b_Ga=p,i={full_id},p={primary_pid},w={src_w},h={src_h},X={x_off},Y={y_off},c={cols},r=1,C=1,z=1,q=2\x1b\\",
            x_off = e.x_off,
            y_off = e.y_off,
            cols = e.cols,
        )?;

        // 2nd placement: overflow into next row (if any)
        if let Some(next_row) = e.overflow_next_row {
            let first_coverage = rgn.ch - e.y_off;
            let overflow = (r.h_px - first_coverage).min(rgn.ch);
            let pattern = select_overflow_pattern(overflow, rgn.ch);

            let (ov_id, ov_w, ov_h) = match pattern {
                PartialPattern::Full => (full_id, src_w, HIGHLIGHT_PNG_HEIGHT),
                PartialPattern::P75 => (p75_id, PATTERN_WIDTH, PATTERN_HEIGHT),
                PartialPattern::P50 => (p50_id, PATTERN_WIDTH, PATTERN_HEIGHT),
                PartialPattern::P25 => (p25_id, PATTERN_WIDTH, PATTERN_HEIGHT),
            };

            debug!("hl overflow: next_row={next_row} overflow={overflow}px pattern={pattern:?}",);

            let overflow_slot = PlacementSlot::OverlayOverflow(tile_idx, rect_idx);
            let overflow_pid = loaded.track_placement(out, overflow_slot, ov_id)?;
            out.queue(cursor::MoveTo(e.col, next_row))?;
            write!(
                out,
                "\x1b_Ga=p,i={ov_id},p={overflow_pid},w={ov_w},h={ov_h},X={x_off},c={cols},r=1,C=1,z=1,q=2\x1b\\",
                x_off = e.x_off,
                cols = e.cols,
            )?;
        }
    }
    Ok(())
}

/// Draw the status bar on the last terminal row.
///
/// `acc_peek`: shows accumulated count like `:56_` when digits are being typed
/// `flash`: transient message (e.g. yank success), cleared on next keypress
pub(super) fn draw_status_bar(
    layout: &Layout,
    scroll: &ScrollState,
    filename: &str,
    acc_peek: Option<u32>,
    flash: Option<&str>,
) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, layout.status_row))?;

    let max_y = scroll.img_h.saturating_sub(scroll.vp_h);
    let pct = if max_y == 0 {
        100
    } else {
        ((scroll.y_offset as u64 * 100) / max_y as u64) as u32
    };

    let total_cols = layout.sidebar_cols + layout.image_cols;

    let middle = if let Some(msg) = flash {
        format!(
            " {} | {} | y={}/{} px  {}%",
            filename, msg, scroll.y_offset, scroll.img_h, pct
        )
    } else if let Some(n) = acc_peek {
        format!(
            " {} | :{n}_ | y={}/{} px  {}%",
            filename, scroll.y_offset, scroll.img_h, pct
        )
    } else {
        format!(
            " {} | y={}/{} px  {}%  [/:search n/N:match Ng:goto j/k d/u ::cmd q:quit]",
            filename, scroll.y_offset, scroll.img_h, pct
        )
    };

    let padded = format!("{:<width$}", middle, width = total_cols as usize);
    let truncated: String = padded.chars().take(total_cols as usize).collect();
    write!(out, "{}", truncated.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;
    out.flush()
}

/// Truncate a string to at most `max_bytes`, respecting UTF-8 char boundaries.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    &s[..s.floor_char_boundary(max_bytes)]
}

/// Draw the inline search status bar: `/query` or `?query`.
pub(super) fn draw_inline_search_bar(
    layout: &Layout,
    query: &str,
    prompt_char: char,
) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, layout.status_row))?;

    let total_cols = (layout.sidebar_cols + layout.image_cols) as usize;

    let line = format!("{prompt_char}{query}");
    let truncated = truncate_str(&line, total_cols);

    write!(
        out,
        "{}",
        format!("{:<width$}", truncated, width = total_cols)
            .on_dark_grey()
            .white()
    )?;
    out.queue(style::ResetColor)?;
    out.flush()
}

/// Loading screen: show "Building… q:quit" in the status bar.
///
/// Called only when the 100ms fast path is exceeded.
/// When `clear_screen` is true, also clears the screen (for initial load when no images exist).
/// During double-buffered reload, pass false to keep the old tiles visible.
pub(super) fn draw_loading_screen(
    layout: &Layout,
    filename: &str,
    clear_screen: bool,
) -> io::Result<()> {
    let mut out = stdout();
    if clear_screen {
        out.queue(terminal::Clear(terminal::ClearType::All))?;
    }
    out.queue(cursor::MoveTo(0, layout.status_row))?;

    let total_cols = (layout.sidebar_cols + layout.image_cols) as usize;
    let msg = format!(" {filename} | Building\u{2026}  q:quit");
    let padded = format!("{:<width$}", msg, width = total_cols);
    let truncated: String = padded.chars().take(total_cols).collect();
    write!(out, "{}", truncated.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;
    out.flush()
}

/// Draw command input bar on the status row (`:input_` prompt).
pub(super) fn draw_command_bar(layout: &Layout, input: &str) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, layout.status_row))?;

    let total_cols = (layout.sidebar_cols + layout.image_cols) as usize;
    let prompt = format!(":{input}_");
    let padded = format!("{:<width$}", prompt, width = total_cols);
    let truncated: String = padded.chars().take(total_cols).collect();
    write!(out, "{}", truncated.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;
    out.flush()
}

/// Send text to the system clipboard via OSC 52.
pub(super) fn send_osc52(text: &str) -> io::Result<()> {
    let encoded = BASE64.encode(text.as_bytes());
    let mut out = stdout();
    write!(out, "\x1b]52;c;{encoded}\x1b\\")?;
    out.flush()
}

// ---------------------------------------------------------------------------
// Terminal theme detection via OSC 11
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTheme {
    Light,
    Dark,
}

/// Detect terminal background brightness via OSC 11 query.
/// Must be called in raw mode. Falls back to Dark on failure.
pub fn detect_terminal_theme(timeout: Duration) -> TerminalTheme {
    match detect_terminal_theme_inner(timeout) {
        Some(theme) => theme,
        None => TerminalTheme::Dark,
    }
}

fn detect_terminal_theme_inner(timeout: Duration) -> Option<TerminalTheme> {
    // Open /dev/tty for read+write
    let mut tty = match std::fs::File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
    {
        Ok(f) => f,
        Err(e) => {
            warn!("cannot open /dev/tty for theme detection, falling back to dark theme: {e}");
            return None;
        }
    };

    // Send OSC 11 query
    if tty.write_all(b"\x1b]11;?\x1b\\").is_err() || tty.flush().is_err() {
        warn!("failed to send OSC 11 query, falling back to dark theme");
        return None;
    }

    // Poll for response with timeout
    let timeout_ms = timeout.as_millis() as i32;
    let fd = tty.as_raw_fd();
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };

    let ready = unsafe { libc::poll(&mut pollfd as *mut _, 1, timeout_ms) };
    if ready <= 0 {
        warn!(
            "OSC 11 query timed out after {}ms, falling back to dark theme",
            timeout.as_millis()
        );
        return None;
    }

    // Read response
    let mut buf = [0u8; 256];
    let n = {
        use std::io::Read;
        match tty.read(&mut buf) {
            Ok(n) => n,
            Err(_) => {
                warn!("failed to read OSC 11 response, falling back to dark theme");
                return None;
            }
        }
    };

    let (r, g, b) = match parse_osc11_response(&buf[..n]) {
        Some(rgb) => rgb,
        None => {
            warn!(
                "failed to parse OSC 11 response: {:?}, falling back to dark theme",
                &buf[..n]
            );
            return None;
        }
    };

    // Approximate luminance for light/dark classification.
    // OSC 11 returns raw RGB without color space information, so we cannot
    // do a strictly correct conversion (no guarantee of sRGB/BT.709/BT.2020,
    // no gamma metadata). We skip sRGB linearization and use BT.709
    // coefficients directly on the raw values. This is technically wrong,
    // but the threshold (0.5) has wide margin for typical backgrounds —
    // dark themes sit around 0.1 and light themes around 0.9 — so the
    // color-space ambiguity never flips the result in practice.
    let rf = r as f64 / 0xFFFF as f64;
    let gf = g as f64 / 0xFFFF as f64;
    let bf = b as f64 / 0xFFFF as f64;
    let luminance = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
    debug!("OSC 11: rgb({r:#06x},{g:#06x},{b:#06x}) luminance={luminance:.3}");

    if luminance > 0.5 {
        Some(TerminalTheme::Light)
    } else {
        Some(TerminalTheme::Dark)
    }
}

/// Parse OSC 11 response: `\x1b]11;rgb:RRRR/GGGG/BBBB\x1b\\` or `...\x07`
///
/// Supports 1-digit (F), 2-digit (FF), and 4-digit (FFFF) hex per channel.
fn parse_osc11_response(buf: &[u8]) -> Option<(u16, u16, u16)> {
    let s = std::str::from_utf8(buf).ok()?;
    let rgb_pos = s.find("rgb:")?;
    let after_rgb = &s[rgb_pos + 4..];

    // Find end: ST (\x1b\\) or BEL (\x07) or end of string
    let end = after_rgb
        .find('\x1b')
        .or_else(|| after_rgb.find('\x07'))
        .unwrap_or(after_rgb.len());
    let channels_str = &after_rgb[..end];

    let parts: Vec<&str> = channels_str.split('/').collect();
    if parts.len() != 3 {
        return None;
    }

    let r = parse_color_channel(parts[0])?;
    let g = parse_color_channel(parts[1])?;
    let b = parse_color_channel(parts[2])?;
    Some((r, g, b))
}

/// Parse a single hex color channel, normalizing to 16-bit (0–0xFFFF).
fn parse_color_channel(s: &str) -> Option<u16> {
    let val = u16::from_str_radix(s, 16).ok()?;
    match s.len() {
        1 => Some(val * 0x1111), // F -> FFFF
        2 => Some(val * 0x0101), // FF -> FFFF
        4 => Some(val),          // FFFF
        _ => None,
    }
}

pub(super) fn check_tty() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    // Only stdout matters. crossterm's `use-dev-tty` reads keyboard from /dev/tty
    // (Unix) or Console API (Windows), so stdin being a pipe is always fine.
    if !io::stdout().is_terminal() {
        anyhow::bail!(
            "mlux viewer requires an interactive terminal.\n\
             \n\
             Supported terminals: Kitty, Ghostty, WezTerm\n\
             To render to a file, use: mlux render <input.md> -o output.png"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_osc11_4digit() {
        // xterm-style 4-digit channels
        let buf = b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\";
        let (r, g, b) = parse_osc11_response(buf).unwrap();
        assert_eq!(r, 0x1e1e);
        assert_eq!(g, 0x1e1e);
        assert_eq!(b, 0x2e2e);
    }

    #[test]
    fn parse_osc11_2digit() {
        let buf = b"\x1b]11;rgb:ff/ff/ff\x1b\\";
        let (r, g, b) = parse_osc11_response(buf).unwrap();
        assert_eq!(r, 0xffff);
        assert_eq!(g, 0xffff);
        assert_eq!(b, 0xffff);
    }

    #[test]
    fn parse_osc11_1digit() {
        let buf = b"\x1b]11;rgb:0/0/0\x07";
        let (r, g, b) = parse_osc11_response(buf).unwrap();
        assert_eq!(r, 0);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn parse_osc11_bel_terminator() {
        let buf = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        let (r, g, b) = parse_osc11_response(buf).unwrap();
        assert_eq!(r, 0xffff);
        assert_eq!(g, 0xffff);
        assert_eq!(b, 0xffff);
    }

    #[test]
    fn parse_osc11_invalid() {
        assert!(parse_osc11_response(b"garbage").is_none());
        assert!(parse_osc11_response(b"\x1b]11;rgb:ff/ff\x1b\\").is_none());
    }

    #[test]
    fn parse_channel_normalization() {
        assert_eq!(parse_color_channel("f"), Some(0xffff));
        assert_eq!(parse_color_channel("0"), Some(0));
        assert_eq!(parse_color_channel("80"), Some(0x8080));
        assert_eq!(parse_color_channel("1e1e"), Some(0x1e1e));
    }

    #[test]
    fn luminance_dark() {
        // Catppuccin Mocha base: #1e1e2e → very dark
        let r = 0x1e1eu16;
        let g = 0x1e1eu16;
        let b = 0x2e2eu16;
        let rf = r as f64 / 0xFFFF as f64;
        let gf = g as f64 / 0xFFFF as f64;
        let bf = b as f64 / 0xFFFF as f64;
        let lum = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
        assert!(lum < 0.5, "luminance {lum} should be < 0.5 for dark bg");
    }

    #[test]
    fn luminance_light() {
        // White background: rgb:ffff/ffff/ffff
        let r = 0xFFFFu16;
        let g = 0xFFFFu16;
        let b = 0xFFFFu16;
        let rf = r as f64 / 0xFFFF as f64;
        let gf = g as f64 / 0xFFFF as f64;
        let bf = b as f64 / 0xFFFF as f64;
        let lum = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
        assert!(lum > 0.5, "luminance {lum} should be > 0.5 for light bg");
    }

    // ----- KGP placement slot tests --------------------------------------
    //
    // These guard the parity between `overlay_slots_for_tile` (used by
    // `collect_new_slots` for Phase 2 pruning) and the actual emission in
    // `place_rects_in_region`. If these diverge, Phase 2 either leaves
    // phantom placements alive or deletes slots that are about to be
    // re-emitted — both visible as flicker or orphaned overlays.

    use crate::frame::HighlightRect;
    use std::collections::HashSet;

    fn rect(y_px: u32, h_px: u32, is_active: bool) -> HighlightRect {
        HighlightRect {
            x_px: 0,
            y_px,
            w_px: 100,
            h_px,
            is_active,
        }
    }

    fn imgs_for_test() -> HighlightImagesCopy {
        HighlightImagesCopy {
            full_id: 200,
            p75_id: 201,
            p50_id: 202,
            p25_id: 203,
            active_full_id: 204,
            active_p75_id: 205,
            active_p50_id: 206,
            active_p25_id: 207,
        }
    }

    /// Collect the slots actually emitted by `place_rects_in_region` for the
    /// given rects/region by inspecting `DisplayState.live_slots` afterwards.
    fn slots_from_emission(rects: &[HighlightRect], rgn: &TileRegion) -> HashSet<PlacementSlot> {
        let mut loaded = DisplayState::new(4);
        let mut out = Vec::<u8>::new();
        let imgs = imgs_for_test();
        place_rects_in_region(&mut out, &mut loaded, 7, rects, &imgs, rgn).unwrap();
        // Only overlay slots for tile 7 should have been inserted.
        let mut set = HashSet::new();
        for slot in [
            PlacementSlot::OverlayPrimary(7, 0),
            PlacementSlot::OverlayOverflow(7, 0),
            PlacementSlot::OverlayPrimary(7, 1),
            PlacementSlot::OverlayOverflow(7, 1),
            PlacementSlot::OverlayPrimary(7, 2),
            PlacementSlot::OverlayOverflow(7, 2),
            PlacementSlot::OverlayPrimary(7, 3),
            PlacementSlot::OverlayOverflow(7, 3),
        ] {
            if loaded.live_slot_image(slot).is_some() {
                set.insert(slot);
            }
        }
        set
    }

    fn slots_from_predicate(rects: &[HighlightRect], rgn: &TileRegion) -> HashSet<PlacementSlot> {
        let mut set = HashSet::new();
        overlay_slots_for_tile(
            &mut set,
            7,
            rects,
            rgn.src_y,
            rgn.src_h,
            rgn.screen_row,
            rgn.max_rows,
            rgn.ch,
        );
        set
    }

    #[test]
    fn slot_parity_mixed_visibility() {
        // ch = 24px (typical cell height). Four rects:
        //   0: fully visible, single row
        //   1: clipped out above src_y (y_px + h_px <= src_y)
        //   2: overflows into next cell (h_px > first_coverage)
        //   3: row exceeds max_rows (clipped out)
        let rgn = TileRegion {
            src_y: 50,
            src_h: 400,
            screen_row: 0,
            max_rows: 10,
            image_col: 0,
            cw: 10,
            ch: 24,
        };
        let rects = vec![
            // 0 — y_px=60, h=10 → screen_y=10, row=0, y_off=10, first_cov=14,
            //     h=10<=14 → primary only
            rect(60, 10, false),
            // 1 — y+h=40 <= src_y=50 → fully clipped
            rect(0, 40, false),
            // 2 — y_px=100, h=40 → screen_y=50, row=2, y_off=2, first_cov=22,
            //     h=40>22 → overflow to row 3
            rect(100, 40, false),
            // 3 — y_px=400, h=20 → screen_y=350, row=14 >= max_rows=10
            rect(400, 20, false),
        ];
        let predicted = slots_from_predicate(&rects, &rgn);
        let emitted = slots_from_emission(&rects, &rgn);
        assert_eq!(
            predicted, emitted,
            "overlay_slots_for_tile must agree with place_rects_in_region"
        );
        // Sanity assertions on the predicted set (serves as a fixture lock-in).
        assert!(emitted.contains(&PlacementSlot::OverlayPrimary(7, 0)));
        assert!(!emitted.contains(&PlacementSlot::OverlayOverflow(7, 0)));
        assert!(emitted.contains(&PlacementSlot::OverlayPrimary(7, 2)));
        assert!(emitted.contains(&PlacementSlot::OverlayOverflow(7, 2)));
        assert!(!emitted.contains(&PlacementSlot::OverlayPrimary(7, 1)));
        assert!(!emitted.contains(&PlacementSlot::OverlayPrimary(7, 3)));
    }

    #[test]
    fn slot_parity_overflow_at_last_row_boundary() {
        // Overflow that would fall past max_rows: primary emitted, overflow
        // slot must NOT be emitted.
        let rgn = TileRegion {
            src_y: 0,
            src_h: 400,
            screen_row: 0,
            max_rows: 2, // rows 0 and 1 only
            image_col: 0,
            cw: 10,
            ch: 24,
        };
        // y_px=36 → row 1, y_off=12, first_coverage=12, h_px=20 > 12
        // next_row=2, not < max_rows (2) → no overflow slot
        let rects = vec![rect(36, 20, false)];
        let predicted = slots_from_predicate(&rects, &rgn);
        let emitted = slots_from_emission(&rects, &rgn);
        assert_eq!(predicted, emitted);
        assert!(emitted.contains(&PlacementSlot::OverlayPrimary(7, 0)));
        assert!(!emitted.contains(&PlacementSlot::OverlayOverflow(7, 0)));
    }

    #[test]
    fn overlay_active_flip_emits_stale_delete_before_new_placement() {
        // Scenario: a visible highlight rect flips is_active from false
        // (yellow, image 200) to true (orange, image 204). The slot identity
        // OverlayPrimary(tile, 0) is unchanged, but the image_id changed, so
        // atomic in-place move does NOT apply — the old placement on image
        // 200 must be explicitly deleted before the new a=p on image 204.
        let rgn = TileRegion {
            src_y: 0,
            src_h: 200,
            screen_row: 0,
            max_rows: 8,
            image_col: 0,
            cw: 10,
            ch: 24,
        };
        let mut loaded = DisplayState::new(4);
        let mut out = Vec::<u8>::new();
        let imgs = imgs_for_test();

        // Frame 1: inactive rect.
        place_rects_in_region(&mut out, &mut loaded, 7, &[rect(0, 20, false)], &imgs, &rgn)
            .unwrap();
        out.clear();

        // Frame 2: same slot, is_active = true → image_id switches 200→204.
        place_rects_in_region(&mut out, &mut loaded, 7, &[rect(0, 20, true)], &imgs, &rgn).unwrap();

        let wire = String::from_utf8(out).unwrap();
        let del_idx = wire
            .find("a=d,d=i,i=200,p=1")
            .expect("stale delete of old image_id must be emitted");
        let place_idx = wire
            .find("a=p,i=204,p=1")
            .expect("new placement on active image_id must be emitted");
        assert!(
            del_idx < place_idx,
            "stale delete must precede new placement; got {wire:?}"
        );
        assert_eq!(
            loaded.live_slot_image(PlacementSlot::OverlayPrimary(7, 0)),
            Some(204),
        );
    }
}
