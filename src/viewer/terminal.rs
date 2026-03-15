//! Terminal I/O layer: raw mode, Kitty Graphics Protocol, status bar, OSC 52.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    ExecutableCommand, QueueableCommand, cursor,
    style::{self, Stylize},
    terminal,
};
use log::{debug, warn};
use std::io::{self, Write, stdout};
use std::os::unix::io::AsRawFd;
use std::time::Duration;

use super::layout::{Layout, ScrollState};
use super::tiles::LoadedTiles;
use crate::tile::VisibleTiles;

const CHUNK_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// RawGuard — Drop で raw mode / alternate screen / 画像削除を確実に復元
// ---------------------------------------------------------------------------

pub(super) struct RawGuard {
    cleaned: bool,
}

impl RawGuard {
    pub(super) fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        stdout().execute(terminal::EnterAlternateScreen)?;
        stdout().execute(cursor::Hide)?;
        Ok(Self { cleaned: false })
    }

    pub(super) fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        let mut out = stdout();
        let _ = write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\");
        let _ = out.execute(cursor::Show);
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

/// PNG データをチャンク分割して送信（a=t: データ転送のみ、表示なし）
pub(super) fn send_image(png_data: &[u8], image_id: u32) -> io::Result<()> {
    let encoded = BASE64.encode(png_data);
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(CHUNK_SIZE)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();

    let mut out = stdout();
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i == chunks.len() - 1;
        let m = if is_last { 0 } else { 1 };
        if i == 0 {
            write!(
                out,
                "\x1b_Ga=t,f=100,i={image_id},t=d,q=2,m={m};{chunk}\x1b\\"
            )?;
        } else {
            write!(out, "\x1b_Gm={m},q=2;{chunk}\x1b\\")?;
        }
    }
    out.flush()
}

/// Raw RGBA データを送信（a=t: データ転送のみ、表示なし）。
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

/// 画像データ+配置を削除
pub(super) fn delete_image(image_id: u32) -> io::Result<()> {
    let mut out = stdout();
    write!(out, "\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")?;
    out.flush()
}

/// 全画像+データ削除
pub(super) fn delete_all_images() -> io::Result<()> {
    let mut out = stdout();
    write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\")?;
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

/// Place tile(s) using Kitty Graphics Protocol.
///
/// `get_id` selects which image ID to use from a `TileImageIds`.
pub(super) fn place_tiles(
    visible: &VisibleTiles,
    loaded: &LoadedTiles,
    layout: &Layout,
    params: &PlaceParams,
    get_id: fn(&super::tiles::TileImageIds) -> u32,
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
            out.queue(cursor::MoveTo(params.start_col, 0))?;
            write!(
                out,
                "\x1b_Ga=p,i={id},x=0,y={src_y},w={w},h={src_h},c={cols},r={rows},C=1,q=2\x1b\\",
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

            out.queue(cursor::MoveTo(params.start_col, 0))?;
            write!(
                out,
                "\x1b_Ga=p,i={id},x=0,y={top_src_y},w={w},h={top_src_h},c={cols},r={top_rows},C=1,q=2\x1b\\",
                id = top_id,
            )?;
            out.queue(cursor::MoveTo(params.start_col, top_rows))?;
            write!(
                out,
                "\x1b_Ga=p,i={id},x=0,y=0,w={w},h={bot_src_h},c={cols},r={bot_rows},C=1,q=2\x1b\\",
                id = bot_id,
            )?;
        }
    }
    out.flush()
}

/// Place content tile(s) based on visible_tiles result.
pub(super) fn place_content_tiles(
    visible: &VisibleTiles,
    loaded: &LoadedTiles,
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
    )
}

/// Place sidebar tile(s) based on the same visible_tiles as content.
pub(super) fn place_sidebar_tiles(
    visible: &VisibleTiles,
    loaded: &LoadedTiles,
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
    )
}

/// Place highlight rectangles using Y sub-cell offset for pixel-precise
/// vertical alignment, with partial-transparency overflow patterns.
///
/// Each rect gets a primary placement at `row = top / ch` with `Y = top % ch`,
/// and optionally a second placement on the next row for overflow coverage.
pub(super) fn place_overlay_rects(
    visible: &VisibleTiles,
    loaded: &LoadedTiles,
    layout: &Layout,
) -> io::Result<()> {
    let imgs = match loaded.highlight_images() {
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
                loaded.overlay_rects(*idx),
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
                loaded.overlay_rects(*top_idx),
                imgs,
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
                loaded.overlay_rects(*bot_idx),
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
///
/// Uses Y sub-cell offset for pixel-precise vertical alignment. When a rect
/// overflows into the next cell row, a second placement with a partial
/// transparency pattern covers the overflow.
fn place_rects_in_region(
    out: &mut impl Write,
    rects: &[crate::highlight::HighlightRect],
    imgs: &super::tiles::HighlightImages,
    rgn: &TileRegion,
) -> io::Result<()> {
    use crate::highlight::{
        HIGHLIGHT_PNG_HEIGHT, HIGHLIGHT_PNG_WIDTH, PATTERN_HEIGHT, PATTERN_WIDTH, PartialPattern,
        select_overflow_pattern,
    };

    for r in rects {
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

        // 1st placement: FULL image with Y sub-cell offset
        out.queue(cursor::MoveTo(col, row))?;
        write!(
            out,
            "\x1b_Ga=p,i={full_id},w={src_w},h={src_h},X={x_off},Y={y_off},c={cols},r=1,C=1,z=1,q=2\x1b\\",
        )?;

        // 2nd placement: overflow into next row (if any)
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

                out.queue(cursor::MoveTo(col, next_row))?;
                write!(
                    out,
                    "\x1b_Ga=p,i={ov_id},w={ov_w},h={ov_h},X={x_off},c={cols},r=1,C=1,z=1,q=2\x1b\\",
                )?;
            }
        }
    }
    Ok(())
}

/// ステータスバーをターミナル最終行に描画。
///
/// `acc_peek`: 数字蓄積中なら `:56_` のように表示
/// `flash`: ヤンク成功等の一時メッセージ（次のキー入力でクリア）
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

/// ローディング画面: 画面クリア + ステータスバーに "Building… q:quit" 表示。
///
/// 100ms の fast path を超えた場合のみ呼ばれる。
pub(super) fn draw_loading_screen(layout: &Layout, filename: &str) -> io::Result<()> {
    let mut out = stdout();
    out.queue(terminal::Clear(terminal::ClearType::All))?;
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
}
