//! Kitty Graphics Protocol spike — validates core technical risks for Phase 4.
//!
//! Usage: cargo run --bin spike_kitty -- <png_path>
//!
//! SPIKE LIMITATION: q=1 でOKレスポンスを抑制しているため、
//! エラーレスポンス(ENOSPC等)がstdinに到達すると crossterm の
//! event::read() がパースエラーになる可能性がある。
//! スパイクではこのエラーを無視して継続する。
//! 本実装では stdin を直接パースして APC レスポンスとキー入力を
//! 区別するカスタムリーダーが必要。

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    ExecutableCommand, QueueableCommand,
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    style::{self, Stylize},
    terminal,
};
use std::io::{self, Write, stdout};
use std::process;

const CHUNK_SIZE: usize = 4096;
const IMAGE_ID: u32 = 1;
const SCROLL_STEP: u32 = 3; // セル単位

// ---------------------------------------------------------------------------
// RawGuard — Drop で raw mode / alternate screen / 画像削除を確実に復元
// ---------------------------------------------------------------------------

struct RawGuard {
    cleaned: bool,
}

impl RawGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        stdout().execute(terminal::EnterAlternateScreen)?;
        stdout().execute(cursor::Hide)?;
        Ok(Self { cleaned: false })
    }

    fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        let mut out = stdout();
        // 画像削除 (a=d, d=A — 全画像+データ削除)
        let _ = write!(out, "\x1b_Ga=d,d=A,q=1\x1b\\");
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
// PNG dimensions — IHDR からサイズ抽出
// ---------------------------------------------------------------------------

fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG signature (8 bytes) + IHDR chunk length (4 bytes) + "IHDR" (4 bytes) + width (4) + height (4)
    if data.len() < 24 {
        return None;
    }
    // Check PNG signature
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
}

// ---------------------------------------------------------------------------
// send_image — base64 チャンク分割送信
// ---------------------------------------------------------------------------

fn send_image(png_data: &[u8], image_id: u32) -> io::Result<()> {
    let encoded = BASE64.encode(png_data);
    let chunks: Vec<&str> = encoded.as_bytes().chunks(CHUNK_SIZE).map(|c| {
        // SAFETY: base64 output is always valid ASCII/UTF-8
        std::str::from_utf8(c).unwrap()
    }).collect();

    let mut out = stdout();

    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i == chunks.len() - 1;
        let m = if is_last { 0 } else { 1 };

        if i == 0 {
            // 最初のチャンク: 全パラメータを含む
            // a=t (小文字) で送信のみ — 表示せずキャッシュに格納
            // 表示は a=p (place_viewport) で行う
            write!(out, "\x1b_Ga=t,f=100,i={image_id},t=d,q=1,m={m};{chunk}\x1b\\")?;
        } else {
            write!(out, "\x1b_Gm={m},q=1;{chunk}\x1b\\")?;
        }
    }
    out.flush()?;

    Ok(())
}

// ---------------------------------------------------------------------------
// place_viewport — a=p でビューポート配置
// ---------------------------------------------------------------------------

fn place_viewport(
    image_id: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let mut out = stdout();
    // カーソルを左上 (1,1) に移動
    out.queue(cursor::MoveTo(0, 0))?;
    // a=p で配置。x=0 固定、y=src_y でスクロール、w,h でクロップ
    // c,r でセル数指定、C=1 でカーソル移動しない、q=1 でOK抑制
    write!(
        out,
        "\x1b_Ga=p,i={image_id},x=0,y={src_y},w={src_w},h={src_h},c={cols},r={rows},C=1,q=1\x1b\\"
    )?;
    out.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// draw_status_bar — 最終行にスクロール位置表示
// ---------------------------------------------------------------------------

fn draw_status_bar(
    row: u16,
    cols: u16,
    y_offset: u32,
    img_height: u32,
    viewport_h: u32,
) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, row))?;
    out.queue(style::SetBackgroundColor(style::Color::DarkGrey))?;
    out.queue(style::SetForegroundColor(style::Color::White))?;

    let max_y = img_height.saturating_sub(viewport_h);
    let pct = if max_y == 0 {
        100
    } else {
        ((y_offset as u64 * 100) / max_y as u64) as u32
    };

    let status = format!(
        " y={y_offset}/{img_height}px  {pct}%  [j/k:scroll  g/G:top/bottom  q:quit]"
    );
    // 行全体をステータスバー色で埋める
    let padded = format!("{:<width$}", status, width = cols as usize);
    write!(out, "{}", padded.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;
    out.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// redraw — 配置削除 → 再配置 → ステータスバー
// ---------------------------------------------------------------------------

fn redraw(
    out: &mut impl Write,
    image_id: u32,
    y_offset: u32,
    vp_w: u32,
    vp_h: u32,
    image_cols: u16,
    image_rows: u16,
    term_rows: u16,
    term_cols: u16,
    img_h: u32,
) -> io::Result<()> {
    // 1. Kitty の a=d,d=i で配置のみ削除（画像データはキャッシュに残す）
    write!(out, "\x1b_Ga=d,d=i,i={image_id},q=1\x1b\\")?;
    out.flush()?;

    // 2. 新しいビューポートを配置
    place_viewport(image_id, y_offset, vp_w, vp_h, image_cols, image_rows)?;

    // 3. ステータスバー再描画
    draw_status_bar(term_rows - 1, term_cols, y_offset, img_h, vp_h)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        // RawGuard の Drop が先に呼ばれるので、ここではターミナルは復元済み
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

fn run() -> io::Result<()> {
    // 1. CLI引数からPNGパスを取得
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <png_path>", args[0]);
        process::exit(1);
    }
    let png_path = &args[1];

    // 2. PNG読み込み + IHDRからサイズ抽出
    let png_data = std::fs::read(png_path).map_err(|e| {
        io::Error::new(e.kind(), format!("Failed to read {png_path}: {e}"))
    })?;
    let (img_w, img_h) = png_dimensions(&png_data).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Not a valid PNG file")
    })?;
    eprintln!("Image: {img_w}x{img_h}px, {} bytes", png_data.len());

    // 3. ターミナルサイズ検出
    let winsize = terminal::window_size()?;
    let term_cols = winsize.columns;
    let term_rows = winsize.rows;
    let pixel_w = winsize.width;
    let pixel_h = winsize.height;

    if pixel_w == 0 || pixel_h == 0 {
        eprintln!(
            "Error: Terminal reported pixel size {}x{} — Kitty graphics requires non-zero pixel dimensions.",
            pixel_w, pixel_h
        );
        eprintln!("Terminal: {}x{} cells", term_cols, term_rows);
        process::exit(1);
    }

    let cell_w = pixel_w / term_cols;
    let cell_h = pixel_h / term_rows;
    eprintln!(
        "Terminal: {}x{} cells, {}x{} px, cell={}x{} px",
        term_cols, term_rows, pixel_w, pixel_h, cell_w, cell_h
    );

    // 4. ビューポート計算
    let status_rows: u16 = 1;
    let image_rows = term_rows.saturating_sub(status_rows);
    let image_cols = term_cols;

    let viewport_w = image_cols as u32 * cell_w as u32;
    let viewport_h = image_rows as u32 * cell_h as u32;
    eprintln!(
        "Viewport: {}x{} cells = {}x{} px",
        image_cols, image_rows, viewport_w, viewport_h
    );

    // ビューポート幅は画像幅に制限
    let vp_w = viewport_w.min(img_w);
    let vp_h = viewport_h.min(img_h);
    let scroll_step_px = SCROLL_STEP as u32 * cell_h as u32;

    // 5. raw mode + alternate screen 開始
    let mut guard = RawGuard::enter()?;

    // 6. PNG全体をチャンク送信
    send_image(&png_data, IMAGE_ID)?;

    // 7. 初回ビューポート配置
    let mut y_offset: u32 = 0;
    place_viewport(IMAGE_ID, y_offset, vp_w, vp_h, image_cols, image_rows)?;
    draw_status_bar(term_rows - 1, term_cols, y_offset, img_h, vp_h)?;

    // 8. イベントループ
    let max_y = img_h.saturating_sub(vp_h);

    loop {
        let ev = event::read()?;
        match ev {
            Event::Key(KeyEvent { code, modifiers, .. }) => {
                match (code, modifiers) {
                    // q or Ctrl-C: 終了
                    (KeyCode::Char('q'), _) => break,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,

                    // j / Down: 下スクロール
                    (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                        y_offset = (y_offset + scroll_step_px).min(max_y);
                    }

                    // k / Up: 上スクロール
                    (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                        y_offset = y_offset.saturating_sub(scroll_step_px);
                    }

                    // g: 先頭
                    (KeyCode::Char('g'), _) => {
                        y_offset = 0;
                    }

                    // G: 末尾
                    (KeyCode::Char('G'), _) => {
                        y_offset = max_y;
                    }

                    _ => continue,
                }

                // 再描画
                // Kitty の a=d,d=i で配置のみ削除（画像データはキャッシュに残す）
                // \x1b[2J (ClearType::All) は Ghostty で画像データごと消えるため使わない
                redraw(&mut stdout(), IMAGE_ID, y_offset, vp_w, vp_h,
                       image_cols, image_rows, term_rows, term_cols, img_h)?;
            }
            Event::Resize(new_cols, new_rows) => {
                // リサイズ時はビューポート再計算（簡易版）
                let new_winsize = terminal::window_size()?;
                let new_image_rows = new_rows.saturating_sub(status_rows);
                let new_cell_h = if new_rows > 0 {
                    new_winsize.height / new_rows
                } else {
                    cell_h
                };
                let new_vp_h = (new_image_rows as u32 * new_cell_h as u32).min(img_h);
                let new_vp_w = (new_cols as u32 * (new_winsize.width / new_cols.max(1)) as u32).min(img_w);

                // max_y を再計算して y_offset をクランプ
                let new_max_y = img_h.saturating_sub(new_vp_h);
                y_offset = y_offset.min(new_max_y);

                redraw(&mut stdout(), IMAGE_ID, y_offset, new_vp_w, new_vp_h,
                       new_cols, new_image_rows, new_rows, new_cols, img_h)?;
            }
            _ => {}
        }
    }

    // 9. クリーンアップ（RawGuard::Drop でも呼ばれるが明示的に）
    guard.cleanup();
    Ok(())
}
