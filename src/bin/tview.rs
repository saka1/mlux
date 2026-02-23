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
//!
//! Kitty response suppression:
//!   All Kitty Graphics Protocol commands use `q=2` (suppress all responses).
//!   Without this, error responses (e.g. ENOENT from oversized images) are
//!   delivered as APC sequences that crossterm misparses as key events,
//!   causing phantom scrolling. `q=2` suppresses both OK and error responses.
//!   Since tview never reads Kitty responses, this is always safe.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    ExecutableCommand, QueueableCommand,
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    style::{self, Stylize},
    terminal,
};
use log::{debug, info};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::process;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tmark::convert::markdown_to_typst;
use typst::layout::PagedDocument;

use tmark::render::compile_document;
use tmark::strip::{
    StripDocument, StripDocumentCache, StripPngs, VisualLine, VisibleStrips,
    extract_visual_lines, generate_sidebar_typst,
};
use tmark::world::TmarkWorld;

const CHUNK_SIZE: usize = 4096;
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
            write!(out, "\x1b_Ga=t,f=100,i={image_id},t=d,q=2,m={m};{chunk}\x1b\\")?;
        } else {
            write!(out, "\x1b_Gm={m},q=2;{chunk}\x1b\\")?;
        }
    }
    out.flush()
}

/// 画像データ+配置を削除
fn delete_image(image_id: u32) -> io::Result<()> {
    let mut out = stdout();
    write!(out, "\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")?;
    out.flush()
}

/// Parameters for placing strip images via Kitty Graphics Protocol.
struct PlaceParams {
    start_col: u16,   // terminal column where placement begins
    num_cols: u16,    // number of terminal columns for the placement
    img_width: u32,   // pixel width of the source image region
}

/// Place strip(s) using Kitty Graphics Protocol.
///
/// `get_id` selects which image ID to use from a `StripImageIds`.
fn place_strips(
    visible: &VisibleStrips,
    loaded: &LoadedStrips,
    layout: &Layout,
    params: &PlaceParams,
    get_id: fn(&StripImageIds) -> u32,
) -> io::Result<()> {
    let mut out = stdout();
    let w = params.img_width;
    let cols = params.num_cols;

    match visible {
        VisibleStrips::Single { idx, src_y, src_h } => {
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
        VisibleStrips::Split {
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

/// Kitty image IDs for a content + sidebar strip pair.
struct StripImageIds {
    content_id: u32,
    sidebar_id: u32,
}

/// Track which strip PNGs are loaded in the terminal, keyed by strip index.
struct LoadedStrips {
    /// strip_index → Kitty image IDs (content + sidebar)
    map: HashMap<usize, StripImageIds>,
    next_id: u32,
}

impl LoadedStrips {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            next_id: 100, // Reserve 1-99 for future use
        }
    }

    /// Ensure a strip (content + sidebar) is loaded in the terminal.
    fn ensure_loaded(
        &mut self,
        strip_doc: &StripDocument,
        cache: &mut StripDocumentCache,
        idx: usize,
    ) -> anyhow::Result<()> {
        if self.map.contains_key(&idx) {
            return Ok(());
        }

        let content_id = self.next_id;
        let sidebar_id = self.next_id + 1;
        self.next_id += 2;

        let pngs = cache.get_or_render(strip_doc, idx)?;
        send_image(&pngs.content, content_id)?;
        send_image(&pngs.sidebar, sidebar_id)?;
        self.map.insert(idx, StripImageIds { content_id, sidebar_id });

        // Evict strips far from current viewport to bound terminal memory
        let to_evict: Vec<usize> = self
            .map
            .keys()
            .filter(|&&k| (k as isize - idx as isize).unsigned_abs() > 4)
            .copied()
            .collect();
        for k in to_evict {
            if let Some(ids) = self.map.remove(&k) {
                let _ = delete_image(ids.content_id);
                let _ = delete_image(ids.sidebar_id);
            }
        }

        Ok(())
    }

    /// Delete all strip placements (content + sidebar, keep image data).
    fn delete_placements(&self) -> io::Result<()> {
        let mut out = stdout();
        for ids in self.map.values() {
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ids.content_id)?;
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ids.sidebar_id)?;
        }
        out.flush()
    }
}

/// Place content strip(s) based on visible_strips result.
fn place_content_strips(
    visible: &VisibleStrips,
    loaded: &LoadedStrips,
    layout: &Layout,
    state: &ViewState,
) -> io::Result<()> {
    place_strips(
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

/// Place sidebar strip(s) based on the same visible_strips as content.
fn place_sidebar_strips(
    visible: &VisibleStrips,
    loaded: &LoadedStrips,
    strip_doc: &StripDocument,
    layout: &Layout,
) -> io::Result<()> {
    place_strips(
        visible,
        loaded,
        layout,
        &PlaceParams {
            start_col: 0,
            num_cols: layout.sidebar_cols,
            img_width: strip_doc.sidebar_width_px(),
        },
        |ids| ids.sidebar_id,
    )
}

/// Full redraw: content strips + sidebar + status bar.
///
/// Ordering: ensure loaded (slow) → delete placements → place new (fast).
fn redraw(
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
        VisibleStrips::Split { top_idx, bot_idx, .. } => {
            loaded.ensure_loaded(strip_doc, cache, *top_idx)?;
            loaded.ensure_loaded(strip_doc, cache, *bot_idx)?;
        }
    }

    // Phase 2: Delete old placements atomically, then place new ones.
    loaded.delete_placements()?;

    // Phase 3: Place content + sidebar + status bar
    place_content_strips(&visible, loaded, layout, state)?;
    place_sidebar_strips(&visible, loaded, strip_doc, layout)?;
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

    // 1. Compile content document
    let content_world = TmarkWorld::new(theme_text, content_text, width_pt);
    let document = compile_document(&content_world)?;

    // 2. Extract visual lines (needed for sidebar generation)
    let visual_lines = extract_visual_lines(&document, PPI);
    let page_height_pt = document.pages[0].frame.size().y.to_pt();

    // 3. Compile sidebar document using visual lines
    let sidebar_doc = build_sidebar_doc(&visual_lines, layout, page_height_pt)?;

    // 4. Build StripDocument with both content + sidebar
    Ok(StripDocument::new(
        &document,
        &sidebar_doc,
        visual_lines,
        strip_height_pt,
        PPI,
    ))
}

fn build_sidebar_doc(
    visual_lines: &[VisualLine],
    layout: &Layout,
    page_height_pt: f64,
) -> anyhow::Result<PagedDocument> {
    let sidebar_width_pt = layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / PPI as f64;
    let sidebar_source = generate_sidebar_typst(
        visual_lines,
        sidebar_width_pt,
        page_height_pt,
    );

    let sidebar_world = TmarkWorld::new_raw(&sidebar_source);
    compile_document(&sidebar_world)
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
///
/// ## in_flight による二重レンダリング防止
///
/// `cache` だけでは TOCTOU (Time-of-Check-to-Time-of-Use) が発生する:
///   1. worker がストリップ N をレンダリング完了 → `res_tx.send()` で結果送信
///   2. main thread の `send_prefetch()` が `cache.contains(N)` を検査 → false
///      (結果は mpsc チャネル内にあるが、まだ `cache.insert()` されていない)
///   3. ストリップ N を再リクエスト → worker が同じストリップを二重レンダリング
///
/// `in_flight` は「送信済み・未受信」のストリップ index を追跡し、この隙間を埋める:
///   - `send_prefetch()`: `in_flight` に insert してからリクエスト送信
///   - `res_rx.try_recv()`: 結果受信時に `in_flight` から remove
///
/// `in_flight` は main thread 専用。worker thread はアクセスしない。
fn send_prefetch(
    tx: &mpsc::Sender<usize>,
    doc: &StripDocument,
    cache: &StripDocumentCache,
    in_flight: &mut HashSet<usize>,
    y_offset: u32,
) {
    let current = (y_offset / doc.strip_height_px()) as usize;
    // Forward 2 + backward 1
    for idx in [current + 1, current + 2, current.wrapping_sub(1)] {
        if idx < doc.strip_count() && !cache.contains(idx) && !in_flight.contains(&idx) {
            debug!("prefetch: requesting strip {idx} (current={current})");
            let _ = tx.send(idx);
            in_flight.insert(idx);
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
        // 4. Build StripDocument (content + sidebar compiled & split)
        info!("building strip document...");
        let strip_doc = build_strip_document(&theme_text, &content_text, &layout)?;

        let img_w = strip_doc.width_px();
        let img_h = strip_doc.total_height_px();
        let (vp_w, vp_h) = vp_dims(&layout, img_w, img_h);
        let mut state = ViewState {
            y_offset: y_offset_carry.min(strip_doc.max_scroll(vp_h)),
            img_h,
            vp_w,
            vp_h,
            filename: filename.clone(),
        };

        let mut cache = StripDocumentCache::new();
        let mut loaded = LoadedStrips::new();

        // 5. thread::scope — prefetch worker + inner event loop
        let exit = thread::scope(|s| -> anyhow::Result<ExitReason> {
            let (req_tx, req_rx) = mpsc::channel::<usize>();
            let (res_tx, res_rx) = mpsc::channel::<(usize, StripPngs)>();

            // Prefetch worker: FIFO — process each request in order.
            //
            // 各リクエストを受信順に処理する。drain-to-latest は使わない:
            // send_prefetch() は [current+1, current+2, current-1] の独立した
            // 複数リクエストを送るため、最後だけ残すと手前のストリップが
            // プリフェッチされず、メインスレッドで同期レンダリングが発生する。
            //
            // worker → main の結果は res_tx/res_rx チャネル経由。
            // main thread が res_rx.try_recv() で受信し cache に格納する。
            let doc = &strip_doc;
            s.spawn(move || {
                debug!("prefetch worker: started");
                while let Ok(idx) = req_rx.recv() {
                    debug!("prefetch worker: rendering strip {idx}");
                    match (doc.render_strip(idx), doc.render_sidebar_strip(idx)) {
                        (Ok(content), Ok(sidebar)) => {
                            debug!("prefetch worker: strip {idx} done (content={}, sidebar={} bytes)", content.len(), sidebar.len());
                            let _ = res_tx.send((idx, StripPngs { content, sidebar }));
                        }
                        (Err(e), _) | (_, Err(e)) => {
                            log::error!("prefetch worker: strip {idx} failed: {e}");
                        }
                    }
                }
                debug!("prefetch worker: channel closed, exiting");
            });

            // in_flight: 「worker に送信済みだが結果未受信」のストリップ index 集合。
            // main thread 専用（worker はアクセスしない）。
            // send_prefetch() で insert、res_rx.try_recv() で remove。
            // cache と合わせてチェックすることで二重レンダリングを防ぐ。
            let mut in_flight: HashSet<usize> = HashSet::new();

            // Initial redraw + prefetch
            redraw(doc, &mut cache, &mut loaded, &layout, &state)?;
            send_prefetch(&req_tx, doc, &cache, &mut in_flight, state.y_offset);

            // Inner event loop
            let mut dirty = false;
            let mut last_render = Instant::now();

            loop {
                // Drain prefetch results into cache.
                // worker が res_tx.send() した結果をノンブロッキングで回収し、
                // cache に格納 + in_flight から除去する。
                while let Ok((idx, pngs)) = res_rx.try_recv() {
                    debug!("main: received prefetched strip {idx} (content={}, sidebar={} bytes)", pngs.content.len(), pngs.sidebar.len());
                    in_flight.remove(&idx);
                    cache.insert(idx, pngs);
                }

                let timeout = if dirty {
                    FRAME_BUDGET.saturating_sub(last_render.elapsed())
                } else {
                    Duration::from_secs(86400)
                };

                if event::poll(timeout)? {
                    let ev = event::read()?;
                    debug!("event: {:?}", ev);
                    match ev {
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
                                    let old = state.y_offset;
                                    state.y_offset = (state.y_offset + scroll_step).min(max_y);
                                    debug!("scroll down: y_offset {} → {} (step={scroll_step}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                // 上スクロール
                                (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                                    let old = state.y_offset;
                                    state.y_offset =
                                        state.y_offset.saturating_sub(scroll_step);
                                    debug!("scroll up: y_offset {} → {} (step={scroll_step}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                // 半画面下
                                (KeyCode::Char('d'), _) => {
                                    let old = state.y_offset;
                                    state.y_offset = (state.y_offset + half_page).min(max_y);
                                    debug!("scroll half-down: y_offset {} → {} (step={half_page}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                // 半画面上
                                (KeyCode::Char('u'), _) => {
                                    let old = state.y_offset;
                                    state.y_offset =
                                        state.y_offset.saturating_sub(half_page);
                                    debug!("scroll half-up: y_offset {} → {} (step={half_page}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                // 先頭
                                (KeyCode::Char('g'), _) => {
                                    let old = state.y_offset;
                                    state.y_offset = 0;
                                    debug!("scroll top: y_offset {} → 0", old);
                                    dirty = true;
                                }
                                // 末尾
                                (KeyCode::Char('G'), _) => {
                                    let old = state.y_offset;
                                    state.y_offset = max_y;
                                    debug!("scroll bottom: y_offset {} → {} (max={max_y})", old, state.y_offset);
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
                    // redraw 直前に追加 drain: event::poll() のブロック中に worker が
                    // 完了した結果を回収し、redraw での同期レンダリングを回避する。
                    // これにより send_prefetch の TOCTOU ウィンドウも縮小する。
                    while let Ok((idx, pngs)) = res_rx.try_recv() {
                        debug!("main: received prefetched strip {idx} (content={}, sidebar={} bytes, pre-redraw)", pngs.content.len(), pngs.sidebar.len());
                        in_flight.remove(&idx);
                        cache.insert(idx, pngs);
                    }
                    redraw(doc, &mut cache, &mut loaded, &layout, &state)?;
                    send_prefetch(&req_tx, doc, &cache, &mut in_flight, state.y_offset);
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
                write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\")?;
                out.flush()?;
                // continue 'outer → new strip_doc + new scope + new worker
            }
        }
    }

    guard.cleanup();
    Ok(())
}
