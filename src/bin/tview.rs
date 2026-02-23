//! tview — TUI Markdown viewer with Kitty Graphics Protocol
//!
//! Usage: cargo run --bin tview -- <markdown_path>
//!
//! Layout:
//!   col 0..sidebar_cols : sidebar image (pixel-precise line numbers)
//!   col sidebar_cols..  : content image viewport
//!   row term_rows-1     : status bar

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    ExecutableCommand, QueueableCommand,
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    style::{self, Stylize},
    terminal,
};
use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::process;
use std::time::{Duration, Instant};

use tmark::convert::markdown_to_typst;
use tmark::render::{
    compile_document, extract_visual_lines, generate_sidebar_typst, render_page_to_png,
};
use tmark::world::TmarkWorld;

const CHUNK_SIZE: usize = 4096;
const CONTENT_IMAGE_ID: u32 = 1;
const SIDEBAR_IMAGE_ID: u32 = 2;
const SCROLL_STEP_CELLS: u32 = 3;
const FRAME_BUDGET: Duration = Duration::from_millis(32); // ~30fps max

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
        // 全画像+データ削除
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
// Layout / ViewState
// ---------------------------------------------------------------------------

struct Layout {
    sidebar_cols: u16,
    image_col: u16,    // 画像領域の開始列 (= sidebar_cols)
    image_cols: u16,   // 画像領域の幅 (= term_cols - sidebar_cols)
    image_rows: u16,   // 画像領域の高さ (= term_rows - 1)
    status_row: u16,   // ステータスバーの行 (= term_rows - 1)
    cell_w: u16,       // ピクセル/セル（幅）
    cell_h: u16,       // ピクセル/セル（高さ）
}

struct ViewState {
    y_offset: u32,   // スクロールオフセット（ピクセル）
    img_w: u32,
    img_h: u32,
    vp_w: u32,       // ビューポート幅（ピクセル）
    vp_h: u32,       // ビューポート高さ（ピクセル）
    sidebar_w: u32,  // サイドバー画像幅（ピクセル）
    sidebar_h: u32,  // サイドバー画像高さ（ピクセル）
    filename: String,
}

fn compute_layout(term_cols: u16, term_rows: u16, pixel_w: u16, pixel_h: u16) -> Layout {
    let sidebar_cols: u16 = 6;
    let image_col = sidebar_cols;
    let image_cols = term_cols.saturating_sub(sidebar_cols);
    let image_rows = term_rows.saturating_sub(1);
    let status_row = term_rows.saturating_sub(1);
    let cell_w = if term_cols > 0 { pixel_w / term_cols } else { 1 };
    let cell_h = if term_rows > 0 { pixel_h / term_rows } else { 1 };
    Layout { sidebar_cols, image_col, image_cols, image_rows, status_row, cell_w, cell_h }
}

fn vp_dims(layout: &Layout, img_w: u32, img_h: u32) -> (u32, u32) {
    let vp_w = (layout.image_cols as u32 * layout.cell_w as u32).min(img_w);
    let vp_h = (layout.image_rows as u32 * layout.cell_h as u32).min(img_h);
    (vp_w, vp_h)
}

// ---------------------------------------------------------------------------
// PNG dimensions — IHDR からサイズ抽出
// ---------------------------------------------------------------------------

fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 {
        return None;
    }
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((w, h))
}

// ---------------------------------------------------------------------------
// Kitty protocol helpers
// ---------------------------------------------------------------------------

/// PNG データをチャンク分割して送信（a=t: データ転送のみ、表示なし）
fn send_image(png_data: &[u8], image_id: u32) -> io::Result<()> {
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
            // a=t: transfer only. a=p で別途配置する。
            write!(out, "\x1b_Ga=t,f=100,i={image_id},t=d,q=1,m={m};{chunk}\x1b\\")?;
        } else {
            write!(out, "\x1b_Gm={m},q=1;{chunk}\x1b\\")?;
        }
    }
    out.flush()
}

/// a=p でコンテンツ画像をビューポート配置。
fn place_content(layout: &Layout, state: &ViewState) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(layout.image_col, 0))?;
    write!(
        out,
        "\x1b_Ga=p,i={id},x=0,y={src_y},w={src_w},h={src_h},c={cols},r={rows},C=1,q=1\x1b\\",
        id = CONTENT_IMAGE_ID,
        src_y = state.y_offset,
        src_w = state.vp_w,
        src_h = state.vp_h,
        cols = layout.image_cols,
        rows = layout.image_rows,
    )?;
    out.flush()
}

/// a=p でサイドバー画像を配置（同一 y_offset で同期スクロール）。
fn place_sidebar(layout: &Layout, state: &ViewState) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, 0))?;
    // サイドバーはコンテンツと同じ高さの画像。同一 y_offset でクロップ。
    let sidebar_vp_h = (layout.image_rows as u32 * layout.cell_h as u32).min(state.sidebar_h);
    let sidebar_y = state.y_offset.min(state.sidebar_h.saturating_sub(sidebar_vp_h));
    write!(
        out,
        "\x1b_Ga=p,i={id},x=0,y={src_y},w={src_w},h={src_h},c={cols},r={rows},C=1,q=1\x1b\\",
        id = SIDEBAR_IMAGE_ID,
        src_y = sidebar_y,
        src_w = state.sidebar_w,
        src_h = sidebar_vp_h,
        cols = layout.sidebar_cols,
        rows = layout.image_rows,
    )?;
    out.flush()
}

/// ステータスバーをターミナル最終行に描画。
fn draw_status_bar(layout: &Layout, state: &ViewState) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, layout.status_row))?;

    let max_y = state.img_h.saturating_sub(state.vp_h);
    let pct = if max_y == 0 {
        100
    } else {
        ((state.y_offset as u64 * 100) / max_y as u64) as u32
    };

    let total_cols = layout.sidebar_cols + layout.image_cols;
    let status = format!(
        " {} | y={}/{} px  {}%  [j/k:scroll  d/u:half  g/G:top/bottom  q:quit]",
        state.filename, state.y_offset, state.img_h, pct
    );
    let padded = format!("{:<width$}", status, width = total_cols as usize);
    write!(out, "{}", padded.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;
    out.flush()
}

/// 配置削除 → サイドバー → コンテンツ → ステータスバー の順で再描画。
///
/// NOTE: \x1b[2J (ClearType::All) は Ghostty で画像データごと消えるため使わない。
/// 配置削除は a=d,d=i (配置のみ削除、データ保持) を使う。
fn redraw(layout: &Layout, state: &ViewState) -> io::Result<()> {
    let mut out = stdout();
    // 両画像の配置を削除（画像データはキャッシュに残す）
    write!(out, "\x1b_Ga=d,d=i,i={},q=1\x1b\\", CONTENT_IMAGE_ID)?;
    write!(out, "\x1b_Ga=d,d=i,i={},q=1\x1b\\", SIDEBAR_IMAGE_ID)?;
    out.flush()?;

    place_sidebar(layout, state)?;
    place_content(layout, state)?;
    draw_status_bar(layout, state)
}

// ---------------------------------------------------------------------------
// Pipeline: compile content + sidebar → send both images
// ---------------------------------------------------------------------------

const PPI: f32 = 144.0;

struct RenderResult {
    content_png: Vec<u8>,
    sidebar_png: Vec<u8>,
    img_w: u32,
    img_h: u32,
    sidebar_w: u32,
    sidebar_h: u32,
}

fn render_pipeline(
    theme_text: &str,
    content_text: &str,
    layout: &Layout,
) -> anyhow::Result<RenderResult> {
    // 1. Content: compile + extract lines + render
    let viewport_px_w = layout.image_cols as f64 * layout.cell_w as f64;
    let width_pt = viewport_px_w * 72.0 / PPI as f64;

    let content_world = TmarkWorld::new(theme_text, content_text, width_pt);
    let document = compile_document(&content_world)?;
    let visual_lines = extract_visual_lines(&document, PPI);
    let content_png = render_page_to_png(&document, PPI)?;

    let (img_w, img_h) = png_dimensions(&content_png)
        .ok_or_else(|| anyhow::anyhow!("rendered content PNG has invalid IHDR"))?;

    // 2. Sidebar: generate typst source + compile + render
    let page_height_pt = if !document.pages.is_empty() {
        document.pages[0].frame.size().y.to_pt()
    } else {
        100.0
    };
    let sidebar_width_pt = layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / PPI as f64;
    let sidebar_source = generate_sidebar_typst(&visual_lines, sidebar_width_pt, page_height_pt);

    let sidebar_world = TmarkWorld::new_raw(&sidebar_source);
    let sidebar_doc = compile_document(&sidebar_world)?;
    let sidebar_png = render_page_to_png(&sidebar_doc, PPI)?;

    let (sidebar_w, sidebar_h) = png_dimensions(&sidebar_png)
        .ok_or_else(|| anyhow::anyhow!("rendered sidebar PNG has invalid IHDR"))?;

    Ok(RenderResult {
        content_png,
        sidebar_png,
        img_w,
        img_h,
        sidebar_w,
        sidebar_h,
    })
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        // RawGuard の Drop が先に呼ばれるので、ここではターミナルは復元済み
        eprintln!("Error: {e:#}");
        process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <markdown_path>", args[0]);
        process::exit(1);
    }
    let md_path = PathBuf::from(&args[1]);
    let filename = md_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // 1. Markdownとテーマを読み込み
    let markdown = std::fs::read_to_string(&md_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", md_path.display()))?;

    let theme_path = PathBuf::from("themes/catppuccin.typ");
    let theme_text = std::fs::read_to_string(&theme_path)
        .map_err(|e| anyhow::anyhow!("failed to read theme {}: {e}", theme_path.display()))?;

    let content_text = markdown_to_typst(&markdown);

    // 2. ターミナルサイズを先に取得してビューポート幅を確定
    let winsize = terminal::window_size()
        .map_err(|e| anyhow::anyhow!("failed to get terminal size: {e}"))?;
    let (term_cols, term_rows) = (winsize.columns, winsize.rows);
    let (pixel_w, pixel_h) = (winsize.width, winsize.height);

    if pixel_w == 0 || pixel_h == 0 {
        anyhow::bail!(
            "terminal pixel size {}x{} is zero — Kitty graphics requires non-zero pixel dimensions",
            pixel_w, pixel_h
        );
    }

    let mut layout = compute_layout(term_cols, term_rows, pixel_w, pixel_h);

    // 3. Render pipeline: content + sidebar
    let result = render_pipeline(&theme_text, &content_text, &layout)?;

    // 4. レイアウト + 初期状態
    let (vp_w, vp_h) = vp_dims(&layout, result.img_w, result.img_h);
    let mut state = ViewState {
        y_offset: 0,
        img_w: result.img_w,
        img_h: result.img_h,
        vp_w,
        vp_h,
        sidebar_w: result.sidebar_w,
        sidebar_h: result.sidebar_h,
        filename,
    };

    // 5. raw mode + alternate screen
    let mut guard = RawGuard::enter()?;

    // 6. PNG データ送信（a=t: 転送のみ）
    send_image(&result.content_png, CONTENT_IMAGE_ID)?;
    send_image(&result.sidebar_png, SIDEBAR_IMAGE_ID)?;

    // 7. 初回描画
    redraw(&layout, &state)?;

    // 8. イベントループ（フレーム予算ベースのスロットリング）
    //
    // 構造: event::poll(timeout) ベース。
    // - dirty/resize_pending が false → タイムアウト無限大でブロック（アイドル時 CPU 0%）
    // - dirty/resize_pending が true → last_render からの経過時間で残り予算を計算し poll
    //   → イベントが来れば state だけ更新して continue（描画スキップ）
    //   → タイムアウトで初めて描画 → 最大 ~30fps
    //
    // キーリピート 30-40Hz でも描画は 30fps 以下に抑えられる。
    let mut dirty = false;
    let mut resize_pending = false;
    let mut last_render = Instant::now();

    loop {
        let timeout = if dirty || resize_pending {
            FRAME_BUDGET.saturating_sub(last_render.elapsed())
        } else {
            Duration::from_secs(86400)
        };

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(KeyEvent { code, modifiers, .. }) => {
                    let max_y = state.img_h.saturating_sub(state.vp_h);
                    let scroll_step = SCROLL_STEP_CELLS * layout.cell_h as u32;
                    let half_page =
                        (layout.image_rows as u32 / 2).max(1) * layout.cell_h as u32;

                    match (code, modifiers) {
                        // 終了
                        (KeyCode::Char('q'), _)
                        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,

                        // 下スクロール
                        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                            state.y_offset = (state.y_offset + scroll_step).min(max_y);
                            dirty = true;
                        }
                        // 上スクロール
                        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                            state.y_offset = state.y_offset.saturating_sub(scroll_step);
                            dirty = true;
                        }
                        // 半画面下
                        (KeyCode::Char('d'), _) => {
                            state.y_offset = (state.y_offset + half_page).min(max_y);
                            dirty = true;
                        }
                        // 半画面上
                        (KeyCode::Char('u'), _) => {
                            state.y_offset = state.y_offset.saturating_sub(half_page);
                            dirty = true;
                        }
                        // 先頭
                        (KeyCode::Char('g'), _) => {
                            state.y_offset = 0;
                            dirty = true;
                        }
                        // 末尾
                        (KeyCode::Char('G'), _) => {
                            state.y_offset = max_y;
                            dirty = true;
                        }

                        _ => {}
                    }
                }

                Event::Resize(new_cols, new_rows) => {
                    let new_winsize = terminal::window_size()?;
                    layout =
                        compute_layout(new_cols, new_rows, new_winsize.width, new_winsize.height);
                    resize_pending = true;
                }

                _ => {}
            }
            continue;
        }

        // poll タイムアウト → フレーム予算消化、描画実行
        if resize_pending {
            let result = render_pipeline(&theme_text, &content_text, &layout)?;
            {
                let mut out = stdout();
                write!(out, "\x1b_Ga=d,d=A,q=1\x1b\\")?;
                out.flush()?;
            }
            send_image(&result.content_png, CONTENT_IMAGE_ID)?;
            send_image(&result.sidebar_png, SIDEBAR_IMAGE_ID)?;

            state.img_w = result.img_w;
            state.img_h = result.img_h;
            state.sidebar_w = result.sidebar_w;
            state.sidebar_h = result.sidebar_h;
            (state.vp_w, state.vp_h) = vp_dims(&layout, result.img_w, result.img_h);
            let new_max_y = state.img_h.saturating_sub(state.vp_h);
            state.y_offset = state.y_offset.min(new_max_y);
            redraw(&layout, &state)?;
            resize_pending = false;
            dirty = false;
        } else if dirty {
            redraw(&layout, &state)?;
            dirty = false;
        }
        last_render = Instant::now();
    }

    guard.cleanup();
    Ok(())
}
