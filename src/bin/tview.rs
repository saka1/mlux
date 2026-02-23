//! tview — TUI Markdown viewer with Kitty Graphics Protocol
//!
//! Usage: cargo run --bin tview -- <markdown_path>
//!
//! Layout:
//!   col 0..sidebar_cols : sidebar image (pixel-precise line numbers)
//!   col sidebar_cols..  : content image viewport (strip-based lazy rendering)
//!   row term_rows-1     : status bar
//!
//! Strip-based rendering:
//!   The document is compiled once with `height: auto`, then the Frame tree
//!   is split into vertical strips. Only visible strips are rendered to PNG,
//!   keeping peak memory proportional to strip size, not document size.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    ExecutableCommand, QueueableCommand,
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    style::{self, Stylize},
    terminal,
};
use log::{debug, info};
use std::collections::HashMap;
use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::process;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tmark::convert::markdown_to_typst;
use tmark::render::{compile_document, render_page_to_png};
use tmark::strip::{StripDocument, StripDocumentCache, VisibleStrips, generate_sidebar_typst};
use tmark::world::TmarkWorld;

const CHUNK_SIZE: usize = 4096;
const SIDEBAR_IMAGE_ID: u32 = 2;
const SCROLL_STEP_CELLS: u32 = 3;
const FRAME_BUDGET: Duration = Duration::from_millis(32); // ~30fps max

const PPI: f32 = 144.0;
const DEFAULT_STRIP_HEIGHT_PT: f64 = 500.0;

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
    img_h: u32,      // ドキュメント高さ（ピクセル）
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

/// 画像データ+配置を削除
fn delete_image(image_id: u32) -> io::Result<()> {
    let mut out = stdout();
    write!(out, "\x1b_Ga=d,d=I,i={image_id},q=1\x1b\\")?;
    out.flush()
}

/// a=p でサイドバー画像を配置（同一 y_offset で同期スクロール）。
fn place_sidebar(layout: &Layout, state: &ViewState) -> io::Result<()> {
    let mut out = stdout();
    out.queue(cursor::MoveTo(0, 0))?;
    // サイドバーはコンテンツと同じ高さの画像。同一 y_offset でクロップ。
    let sidebar_vp_h = (layout.image_rows as u32 * layout.cell_h as u32).min(state.sidebar_h);
    let sidebar_y = state.y_offset.min(state.sidebar_h.saturating_sub(sidebar_vp_h));
    // ドキュメントがターミナルより短い場合、実際のピクセル高さに応じた行数にする
    let sidebar_rows = ((sidebar_vp_h as f64) / (layout.cell_h as f64))
        .ceil()
        .min(layout.image_rows as f64) as u16;
    let sidebar_rows = sidebar_rows.max(1);
    write!(
        out,
        "\x1b_Ga=p,i={id},x=0,y={src_y},w={src_w},h={src_h},c={cols},r={rows},C=1,q=1\x1b\\",
        id = SIDEBAR_IMAGE_ID,
        src_y = sidebar_y,
        src_w = state.sidebar_w,
        src_h = sidebar_vp_h,
        cols = layout.sidebar_cols,
        rows = sidebar_rows,
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

// ---------------------------------------------------------------------------
// Strip-aware content display
// ---------------------------------------------------------------------------

/// Track which strip PNGs are loaded in the terminal, keyed by strip index.
struct LoadedStrips {
    /// strip_index → Kitty image_id
    map: HashMap<usize, u32>,
    next_id: u32,
}

impl LoadedStrips {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            next_id: 100, // Reserve 1-99 for sidebar etc.
        }
    }

    /// Ensure a strip is loaded in the terminal. Returns its Kitty image ID.
    fn ensure_loaded(
        &mut self,
        strip_doc: &StripDocument,
        cache: &mut StripDocumentCache,
        idx: usize,
    ) -> anyhow::Result<u32> {
        if let Some(&id) = self.map.get(&idx) {
            return Ok(id);
        }

        let id = self.next_id;
        self.next_id += 1;

        let png = cache.get_or_render(strip_doc, idx)?;
        send_image(png, id)?;
        self.map.insert(idx, id);

        // Evict strips far from current viewport to bound terminal memory
        let to_evict: Vec<usize> = self
            .map
            .keys()
            .filter(|&&k| (k as isize - idx as isize).unsigned_abs() > 4)
            .copied()
            .collect();
        for k in to_evict {
            if let Some(old_id) = self.map.remove(&k) {
                let _ = delete_image(old_id);
            }
        }

        Ok(id)
    }

    /// Delete all content strip placements (keep image data).
    fn delete_placements(&self) -> io::Result<()> {
        let mut out = stdout();
        for &id in self.map.values() {
            write!(out, "\x1b_Ga=d,d=i,i={id},q=1\x1b\\")?;
        }
        out.flush()
    }
}

/// Place content strip(s) based on visible_strips result.
///
/// Ordering: render + send FIRST, then delete old placements + place new ones.
/// This keeps the old image visible during rendering, avoiding blank flashes.
fn place_content_strips(
    strip_doc: &StripDocument,
    cache: &mut StripDocumentCache,
    loaded: &mut LoadedStrips,
    layout: &Layout,
    state: &ViewState,
) -> anyhow::Result<()> {
    let visible = strip_doc.visible_strips(state.y_offset, state.vp_h);

    // Phase 1: Ensure all needed strips are rendered and sent to the terminal.
    // Old placements remain visible during this potentially slow step.
    match &visible {
        VisibleStrips::Single { idx, .. } => {
            loaded.ensure_loaded(strip_doc, cache, *idx)?;
        }
        VisibleStrips::Split {
            top_idx, bot_idx, ..
        } => {
            loaded.ensure_loaded(strip_doc, cache, *top_idx)?;
            loaded.ensure_loaded(strip_doc, cache, *bot_idx)?;
        }
    }

    // Phase 2: Delete old placements and place new ones atomically.
    // All image data is already in the terminal, so this is instantaneous.
    loaded.delete_placements()?;
    let mut out = stdout();
    write!(out, "\x1b_Ga=d,d=i,i={},q=1\x1b\\", SIDEBAR_IMAGE_ID)?;

    match visible {
        VisibleStrips::Single { idx, src_y, src_h } => {
            let id = *loaded.map.get(&idx).unwrap();
            // Compute rows from pixel height to maintain 1:1 scale.
            // At document end, src_h < vp_h → fewer rows (background shows below).
            let rows = ((src_h as f64) / (layout.cell_h as f64))
                .ceil()
                .min(layout.image_rows as f64) as u16;
            let rows = rows.max(1);
            out.queue(cursor::MoveTo(layout.image_col, 0))?;
            write!(
                out,
                "\x1b_Ga=p,i={id},x=0,y={src_y},w={vp_w},h={src_h},c={cols},r={rows},C=1,q=1\x1b\\",
                vp_w = state.vp_w,
                cols = layout.image_cols,
            )?;
        }
        VisibleStrips::Split {
            top_idx,
            top_src_y,
            top_src_h,
            bot_idx,
            bot_src_h,
        } => {
            let top_id = *loaded.map.get(&top_idx).unwrap();
            let bot_id = *loaded.map.get(&bot_idx).unwrap();

            // Compute rows from pixel heights for correct 1:1 scaling.
            // round() minimizes scaling error; clamp avoids r=0 (Kitty auto-size).
            let top_rows = (top_src_h as f64 / layout.cell_h as f64).round() as u16;
            let top_rows = top_rows.clamp(1, layout.image_rows.saturating_sub(1));
            let bot_rows = layout.image_rows.saturating_sub(top_rows);

            // Top strip
            out.queue(cursor::MoveTo(layout.image_col, 0))?;
            write!(
                out,
                "\x1b_Ga=p,i={top_id},x=0,y={top_src_y},w={vp_w},h={top_src_h},c={cols},r={top_rows},C=1,q=1\x1b\\",
                vp_w = state.vp_w,
                cols = layout.image_cols,
            )?;
            // Bottom strip
            out.queue(cursor::MoveTo(layout.image_col, top_rows))?;
            write!(
                out,
                "\x1b_Ga=p,i={bot_id},x=0,y=0,w={vp_w},h={bot_src_h},c={cols},r={bot_rows},C=1,q=1\x1b\\",
                vp_w = state.vp_w,
                cols = layout.image_cols,
            )?;
        }
    }
    out.flush()?;

    Ok(())
}

/// Full redraw: content strips + sidebar + status bar.
fn redraw(
    strip_doc: &StripDocument,
    cache: &mut StripDocumentCache,
    loaded: &mut LoadedStrips,
    layout: &Layout,
    state: &ViewState,
) -> anyhow::Result<()> {
    place_content_strips(strip_doc, cache, loaded, layout, state)?;
    place_sidebar(layout, state)?;
    draw_status_bar(layout, state)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pipeline: build StripDocument + sidebar
// ---------------------------------------------------------------------------

fn build_strip_document(
    theme_text: &str,
    content_text: &str,
    layout: &Layout,
) -> anyhow::Result<StripDocument> {
    let viewport_px_w = layout.image_cols as f64 * layout.cell_w as f64;
    let width_pt = viewport_px_w * 72.0 / PPI as f64;

    // Strip must be at least as tall as viewport to avoid scaling artifacts.
    // When strip_height < vp_h, Split mode can't fill the viewport from two
    // strips, causing Kitty's r parameter to stretch the content.
    let vp_height_pt = layout.image_rows as f64 * layout.cell_h as f64 * 72.0 / PPI as f64;
    let strip_height_pt = DEFAULT_STRIP_HEIGHT_PT.max(vp_height_pt);
    info!(
        "strip_height: {}pt (vp={}pt, default={}pt)",
        strip_height_pt, vp_height_pt, DEFAULT_STRIP_HEIGHT_PT
    );

    let content_world = TmarkWorld::new(theme_text, content_text, width_pt);
    let document = compile_document(&content_world)?;

    Ok(StripDocument::new(
        &document,
        strip_height_pt,
        PPI,
    ))
}

fn build_sidebar(
    strip_doc: &StripDocument,
    layout: &Layout,
) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let sidebar_width_pt = layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / PPI as f64;
    let sidebar_source = generate_sidebar_typst(
        &strip_doc.visual_lines,
        sidebar_width_pt,
        strip_doc.page_height_pt(),
    );

    let sidebar_world = TmarkWorld::new_raw(&sidebar_source);
    let sidebar_doc = compile_document(&sidebar_world)?;
    let sidebar_png = render_page_to_png(&sidebar_doc, PPI)?;

    let (w, h) = png_dimensions(&sidebar_png)
        .ok_or_else(|| anyhow::anyhow!("rendered sidebar PNG has invalid IHDR"))?;

    Ok((sidebar_png, w, h))
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    env_logger::init();

    if let Err(e) = run() {
        // RawGuard の Drop が先に呼ばれるので、ここではターミナルは復元済み
        eprintln!("Error: {e:#}");
        process::exit(1);
    }
}

/// Why the event loop exited the inner `thread::scope`.
enum ExitReason {
    Quit,
    Resize { new_cols: u16, new_rows: u16 },
}

/// Request prefetch of strips adjacent to the current viewport.
///
/// Sends strip indices for 2 strips ahead and 1 behind the current position.
/// The worker uses drain-to-latest, so rapid-fire requests are cheap.
fn send_prefetch(
    tx: &mpsc::Sender<usize>,
    doc: &StripDocument,
    cache: &StripDocumentCache,
    y_offset: u32,
) {
    let current = (y_offset / doc.strip_height_px()) as usize;
    // Forward 2 + backward 1
    for idx in [current + 1, current + 2, current.wrapping_sub(1)] {
        if idx < doc.strip_count() && !cache.contains(idx) {
            debug!("prefetch: requesting strip {idx} (current={current})");
            let _ = tx.send(idx);
        }
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

    // 3. raw mode + alternate screen (maintained across rebuilds)
    let mut guard = RawGuard::enter()?;

    let mut layout = compute_layout(term_cols, term_rows, pixel_w, pixel_h);
    let mut y_offset_carry: u32 = 0;

    // Outer loop: each iteration builds a new StripDocument (initial + resize)
    'outer: loop {
        // 4. Build StripDocument + sidebar
        info!("building strip document...");
        let strip_doc = build_strip_document(&theme_text, &content_text, &layout)?;
        let (sidebar_png, sidebar_w, sidebar_h) = build_sidebar(&strip_doc, &layout)?;

        let img_w = strip_doc.width_px();
        let img_h = strip_doc.total_height_px();
        let (vp_w, vp_h) = vp_dims(&layout, img_w, img_h);
        let mut state = ViewState {
            y_offset: y_offset_carry.min(strip_doc.max_scroll(vp_h)),
            img_h,
            vp_w,
            vp_h,
            sidebar_w,
            sidebar_h,
            filename: filename.clone(),
        };

        send_image(&sidebar_png, SIDEBAR_IMAGE_ID)?;
        let mut cache = StripDocumentCache::new();
        let mut loaded = LoadedStrips::new();

        // 5. thread::scope — prefetch worker + inner event loop
        let exit = thread::scope(|s| -> anyhow::Result<ExitReason> {
            let (req_tx, req_rx) = mpsc::channel::<usize>();
            let (res_tx, res_rx) = mpsc::channel::<(usize, Vec<u8>)>();

            // Prefetch worker: drain-to-latest pattern
            let doc = &strip_doc;
            s.spawn(move || {
                debug!("prefetch worker: started");
                while let Ok(mut idx) = req_rx.recv() {
                    // Rapid scroll: only process the latest request
                    let mut skipped = 0u32;
                    while let Ok(newer) = req_rx.try_recv() {
                        idx = newer;
                        skipped += 1;
                    }
                    if skipped > 0 {
                        debug!("prefetch worker: drained {skipped} stale, rendering strip {idx}");
                    } else {
                        debug!("prefetch worker: rendering strip {idx}");
                    }
                    match doc.render_strip(idx) {
                        Ok(png) => {
                            debug!("prefetch worker: strip {idx} done ({} bytes)", png.len());
                            let _ = res_tx.send((idx, png));
                        }
                        Err(e) => {
                            log::error!("prefetch worker: strip {idx} failed: {e}");
                        }
                    }
                }
                debug!("prefetch worker: channel closed, exiting");
            });

            // Initial redraw + prefetch
            redraw(doc, &mut cache, &mut loaded, &layout, &state)?;
            send_prefetch(&req_tx, doc, &cache, state.y_offset);

            // Inner event loop
            let mut dirty = false;
            let mut last_render = Instant::now();

            loop {
                // Drain prefetch results into cache
                while let Ok((idx, png)) = res_rx.try_recv() {
                    debug!("main: received prefetched strip {idx} ({} bytes)", png.len());
                    cache.insert(idx, png);
                }

                let timeout = if dirty {
                    FRAME_BUDGET.saturating_sub(last_render.elapsed())
                } else {
                    Duration::from_secs(86400)
                };

                if event::poll(timeout)? {
                    match event::read()? {
                        Event::Key(KeyEvent { code, modifiers, .. }) => {
                            let max_y = doc.max_scroll(state.vp_h);
                            let scroll_step = SCROLL_STEP_CELLS * layout.cell_h as u32;
                            let half_page =
                                (layout.image_rows as u32 / 2).max(1) * layout.cell_h as u32;

                            match (code, modifiers) {
                                // 終了
                                (KeyCode::Char('q'), _)
                                | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                    return Ok(ExitReason::Quit);
                                    // req_tx dropped → worker exits → scope joins
                                }

                                // 下スクロール
                                (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                                    state.y_offset = (state.y_offset + scroll_step).min(max_y);
                                    dirty = true;
                                }
                                // 上スクロール
                                (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                                    state.y_offset =
                                        state.y_offset.saturating_sub(scroll_step);
                                    dirty = true;
                                }
                                // 半画面下
                                (KeyCode::Char('d'), _) => {
                                    state.y_offset = (state.y_offset + half_page).min(max_y);
                                    dirty = true;
                                }
                                // 半画面上
                                (KeyCode::Char('u'), _) => {
                                    state.y_offset =
                                        state.y_offset.saturating_sub(half_page);
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
                            return Ok(ExitReason::Resize { new_cols, new_rows });
                            // req_tx dropped → worker exits → scope joins
                        }

                        _ => {}
                    }
                    continue;
                }

                // poll timeout → frame budget elapsed, execute redraw
                if dirty {
                    redraw(doc, &mut cache, &mut loaded, &layout, &state)?;
                    send_prefetch(&req_tx, doc, &cache, state.y_offset);
                    cache.evict_distant(
                        (state.y_offset / doc.strip_height_px()) as usize,
                        4,
                    );
                    dirty = false;
                }
                last_render = Instant::now();
            }
            // req_tx dropped here → worker recv() gets Err → worker exits → scope joins
        })?;

        match exit {
            ExitReason::Quit => break 'outer,
            ExitReason::Resize { new_cols, new_rows } => {
                y_offset_carry = state.y_offset;
                // Delete all images, then rebuild in next outer iteration
                debug!("resize: rebuilding strip document and sidebar");
                let new_winsize = terminal::window_size()?;
                layout = compute_layout(
                    new_cols,
                    new_rows,
                    new_winsize.width,
                    new_winsize.height,
                );
                let mut out = stdout();
                write!(out, "\x1b_Ga=d,d=A,q=1\x1b\\")?;
                out.flush()?;
                // continue 'outer → new strip_doc + new scope + new worker
            }
        }
    }

    guard.cleanup();
    Ok(())
}
