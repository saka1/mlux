//! Terminal I/O layer: raw mode, Kitty Graphics Protocol, status bar, OSC 52.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    ExecutableCommand, QueueableCommand, cursor,
    style::{self, Stylize},
    terminal,
};
use std::io::{self, Write, stdout};

use super::state::{Layout, LoadedTiles, ViewState};
use crate::tile::{TiledDocument, VisibleTiles};

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

/// Place tile(s) using Kitty Graphics Protocol.
///
/// `get_id` selects which image ID to use from a `TileImageIds`.
pub(super) fn place_tiles(
    visible: &VisibleTiles,
    loaded: &LoadedTiles,
    layout: &Layout,
    params: &PlaceParams,
    get_id: fn(&super::state::TileImageIds) -> u32,
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

            let top_rows = (*top_src_h as f64 / layout.cell_h as f64).round() as u16;
            let top_rows = top_rows.clamp(1, layout.image_rows.saturating_sub(1));
            let bot_rows = layout.image_rows.saturating_sub(top_rows);

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
    state: &ViewState,
) -> io::Result<()> {
    place_tiles(
        visible,
        loaded,
        layout,
        &PlaceParams {
            start_col: layout.image_col,
            num_cols: layout.image_cols,
            img_width: state.vp_w,
        },
        |ids| ids.content_id,
    )
}

/// Place sidebar tile(s) based on the same visible_tiles as content.
pub(super) fn place_sidebar_tiles(
    visible: &VisibleTiles,
    loaded: &LoadedTiles,
    tiled_doc: &TiledDocument,
    layout: &Layout,
) -> io::Result<()> {
    place_tiles(
        visible,
        loaded,
        layout,
        &PlaceParams {
            start_col: 0,
            num_cols: layout.sidebar_cols,
            img_width: tiled_doc.sidebar_width_px(),
        },
        |ids| ids.sidebar_id,
    )
}

/// ステータスバーをターミナル最終行に描画。
///
/// `acc_peek`: 数字蓄積中なら `:56_` のように表示
/// `flash`: ヤンク成功等の一時メッセージ（次のキー入力でクリア）
pub(super) fn draw_status_bar(
    layout: &Layout,
    state: &ViewState,
    acc_peek: Option<u32>,
    flash: Option<&str>,
) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, layout.status_row))?;

    let max_y = state.img_h.saturating_sub(state.vp_h);
    let pct = if max_y == 0 {
        100
    } else {
        ((state.y_offset as u64 * 100) / max_y as u64) as u32
    };

    let total_cols = layout.sidebar_cols + layout.image_cols;

    let middle = if let Some(msg) = flash {
        format!(
            " {} | {} | y={}/{} px  {}%",
            state.filename, msg, state.y_offset, state.img_h, pct
        )
    } else if let Some(n) = acc_peek {
        format!(
            " {} | :{n}_ | y={}/{} px  {}%",
            state.filename, state.y_offset, state.img_h, pct
        )
    } else {
        format!(
            " {} | y={}/{} px  {}%  [/:search n/N:match Ng:goto j/k d/u ::cmd q:quit]",
            state.filename, state.y_offset, state.img_h, pct
        )
    };

    let padded = format!("{:<width$}", middle, width = total_cols as usize);
    write!(out, "{}", padded.on_dark_grey().white())?;
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
    write!(out, "{}", padded.on_dark_grey().white())?;
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
