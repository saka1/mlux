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
use std::time::{Duration, Instant};

use super::layout::{Layout, ScrollState};

// ---------------------------------------------------------------------------
// RawGuard — restores raw mode / alternate screen / image cleanup on Drop
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

/// Delete all images and data
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
}
