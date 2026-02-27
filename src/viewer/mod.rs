//! Terminal Markdown viewer with Kitty Graphics Protocol
//!
//! Layout:
//!   col 0..sidebar_cols : sidebar image (pixel-precise line numbers)
//!   col sidebar_cols..  : content image viewport (tile-based lazy rendering)
//!   row term_rows-1     : status bar
//!
//! Tile-based rendering:
//!   The document is compiled once with `height: auto`, then the Frame tree
//!   is split into vertical tiles. Only visible tiles are rendered to PNG,
//!   keeping peak memory proportional to tile size, not document size.
//!
//! Kitty response suppression:
//!   All Kitty Graphics Protocol commands use `q=2` (suppress all responses).
//!   Without this, error responses (e.g. ENOENT from oversized images) are
//!   delivered as APC sequences that crossterm misparses as key events,
//!   causing phantom scrolling. `q=2` suppresses both OK and error responses.
//!   Since the viewer never reads Kitty responses, this is always safe.

mod input;
mod pipeline;
mod state;
mod terminal;

use crossterm::{
    event::{self, Event},
    terminal as crossterm_terminal,
};
use log::{debug, info};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::convert::markdown_to_typst_with_map;
use crate::tile::{TiledDocumentCache, TilePngs, yank_exact, yank_lines};
use crate::world::FontCache;

use input::{Action, InputAccumulator, map_key_event};
use state::{ExitReason, LoadedTiles, ViewState};

const SCROLL_STEP_CELLS: u32 = 3;
const FRAME_BUDGET: Duration = Duration::from_millis(32); // ~30fps max

/// Run the terminal viewer.
///
/// `md_path` is the Markdown file to display.
/// `theme` is a theme name (loaded from `themes/{theme}.typ`).
pub fn run(md_path: PathBuf, theme: String) -> anyhow::Result<()> {
    let filename = md_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    terminal::check_tty()?;

    // 1. Markdownとテーマを読み込み
    let markdown = std::fs::read_to_string(&md_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", md_path.display()))?;

    let theme_path = PathBuf::from(format!("themes/{}.typ", theme));
    let theme_text = std::fs::read_to_string(&theme_path)
        .map_err(|e| anyhow::anyhow!("failed to read theme {}: {e}", theme_path.display()))?;

    let (content_text, source_map) = markdown_to_typst_with_map(&markdown);

    // 2. ターミナルサイズを先に取得してビューポート幅を確定
    let winsize = crossterm_terminal::window_size()
        .map_err(|e| anyhow::anyhow!("failed to get terminal size: {e}"))?;
    let (term_cols, term_rows) = (winsize.columns, winsize.rows);
    let (pixel_w, pixel_h) = (winsize.width, winsize.height);

    if pixel_w == 0 || pixel_h == 0 {
        anyhow::bail!(
            "terminal pixel size {}x{} is zero — Kitty graphics requires non-zero pixel dimensions",
            pixel_w, pixel_h
        );
    }

    // 3. Font cache (one-time filesystem scan, shared across rebuilds)
    let font_cache = FontCache::new();

    // 4. raw mode + alternate screen (maintained across rebuilds)
    let mut guard = terminal::RawGuard::enter()?;

    let mut layout = state::compute_layout(term_cols, term_rows, pixel_w, pixel_h);
    let mut y_offset_carry: u32 = 0;

    // Outer loop: each iteration builds a new TiledDocument (initial + resize)
    'outer: loop {
        // 5. Build TiledDocument (content + sidebar compiled & split)
        info!("building tiled document...");
        let tiled_doc = pipeline::build_tiled_document(
            &theme_text, &content_text, &markdown, &source_map, &layout, &font_cache,
        )?;

        let img_w = tiled_doc.width_px();
        let img_h = tiled_doc.total_height_px();
        let (vp_w, vp_h) = state::vp_dims(&layout, img_w, img_h);
        let mut state = ViewState {
            y_offset: y_offset_carry.min(tiled_doc.max_scroll(vp_h)),
            img_h,
            vp_w,
            vp_h,
            filename: filename.clone(),
        };

        let mut cache = TiledDocumentCache::new();
        let mut loaded = LoadedTiles::new();

        // 6. thread::scope — prefetch worker + inner event loop
        let exit = thread::scope(|s| -> anyhow::Result<ExitReason> {
            let (req_tx, req_rx) = mpsc::channel::<usize>();
            let (res_tx, res_rx) = mpsc::channel::<(usize, TilePngs)>();

            // Prefetch worker: FIFO — process each request in order.
            //
            // 各リクエストを受信順に処理する。drain-to-latest は使わない:
            // send_prefetch() は [current+1, current+2, current-1] の独立した
            // 複数リクエストを送るため、最後だけ残すと手前のタイルが
            // プリフェッチされず、メインスレッドで同期レンダリングが発生する。
            //
            // worker → main の結果は res_tx/res_rx チャネル経由。
            // main thread が res_rx.try_recv() で受信し cache に格納する。
            let doc = &tiled_doc;
            s.spawn(move || {
                debug!("prefetch worker: started");
                while let Ok(idx) = req_rx.recv() {
                    debug!("prefetch worker: rendering tile {idx}");
                    let render_start = Instant::now();
                    match (doc.render_tile(idx), doc.render_sidebar_tile(idx)) {
                        (Ok(content), Ok(sidebar)) => {
                            debug!("prefetch worker: tile {idx} done in {:.1}ms (content={}, sidebar={} bytes)", render_start.elapsed().as_secs_f64() * 1000.0, content.len(), sidebar.len());
                            let _ = res_tx.send((idx, TilePngs { content, sidebar }));
                        }
                        (Err(e), _) | (_, Err(e)) => {
                            log::error!("prefetch worker: tile {idx} failed: {e}");
                        }
                    }
                }
                debug!("prefetch worker: channel closed, exiting");
            });

            // in_flight: 「worker に送信済みだが結果未受信」のタイル index 集合。
            // main thread 専用（worker はアクセスしない）。
            // send_prefetch() で insert、res_rx.try_recv() で remove。
            // cache と合わせてチェックすることで二重レンダリングを防ぐ。
            let mut in_flight: HashSet<usize> = HashSet::new();

            // Vim-style number prefix accumulator
            let mut acc = InputAccumulator::new();
            // Flash message (e.g., "Yanked L56"), cleared on next keypress
            let mut flash_msg: Option<String> = None;

            // Initial redraw + prefetch
            self::state::redraw(doc, &mut cache, &mut loaded, &layout, &state, acc.peek(), None)?;
            self::state::send_prefetch(&req_tx, doc, &cache, &mut in_flight, state.y_offset);

            // Inner event loop
            let mut dirty = false;
            let mut last_render = Instant::now();

            loop {
                // Drain prefetch results into cache.
                while let Ok((idx, pngs)) = res_rx.try_recv() {
                    debug!("main: received prefetched tile {idx} (content={}, sidebar={} bytes)", pngs.content.len(), pngs.sidebar.len());
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

                    // Clear flash message on any keypress
                    let had_flash = flash_msg.is_some();
                    flash_msg = None;

                    match ev {
                        Event::Key(key_event) => {
                            let max_y = doc.max_scroll(state.vp_h);
                            let scroll_step = SCROLL_STEP_CELLS * layout.cell_h as u32;
                            let half_page =
                                (layout.image_rows as u32 / 2).max(1) * layout.cell_h as u32;

                            match map_key_event(key_event, &mut acc) {
                                Some(Action::Quit) => {
                                    return Ok(ExitReason::Quit);
                                }

                                Some(Action::CancelInput) => {
                                    terminal::draw_status_bar(&layout, &state, None, None)?;
                                }

                                Some(Action::Digit) => {
                                    terminal::draw_status_bar(&layout, &state, acc.peek(), None)?;
                                }

                                Some(Action::ScrollDown(count)) => {
                                    let old = state.y_offset;
                                    state.y_offset = (state.y_offset + count * scroll_step).min(max_y);
                                    debug!("scroll down: y_offset {} → {} (count={count}, step={scroll_step}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                Some(Action::ScrollUp(count)) => {
                                    let old = state.y_offset;
                                    state.y_offset =
                                        state.y_offset.saturating_sub(count * scroll_step);
                                    debug!("scroll up: y_offset {} → {} (count={count}, step={scroll_step}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                Some(Action::HalfPageDown(count)) => {
                                    let old = state.y_offset;
                                    state.y_offset = (state.y_offset + count * half_page).min(max_y);
                                    debug!("scroll half-down: y_offset {} → {} (count={count}, step={half_page}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                Some(Action::HalfPageUp(count)) => {
                                    let old = state.y_offset;
                                    state.y_offset =
                                        state.y_offset.saturating_sub(count * half_page);
                                    debug!("scroll half-up: y_offset {} → {} (count={count}, step={half_page}, max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }

                                Some(Action::JumpToTop) => {
                                    let old = state.y_offset;
                                    state.y_offset = 0;
                                    debug!("scroll top: y_offset {} → 0", old);
                                    dirty = true;
                                }
                                Some(Action::JumpToBottom) => {
                                    let old = state.y_offset;
                                    state.y_offset = max_y;
                                    debug!("scroll bottom: y_offset {} → {} (max={max_y})", old, state.y_offset);
                                    dirty = true;
                                }
                                Some(Action::JumpToLine(n)) => {
                                    let old = state.y_offset;
                                    self::state::jump_to_visual_line(&mut state, &doc.visual_lines, max_y, n);
                                    debug!("jump to line {n}: y_offset {} → {}", old, state.y_offset);
                                    dirty = true;
                                }

                                // 精密ヤンク (y): コードブロックでは1行、他はブロック全体
                                Some(Action::YankExactPrompt) => {
                                    flash_msg = Some("Type Ny to yank line N".into());
                                    terminal::draw_status_bar(&layout, &state, acc.peek(), flash_msg.as_deref())?;
                                }
                                Some(Action::YankExact(n)) => {
                                    let vl_idx = (n as usize).saturating_sub(1);
                                    if vl_idx >= doc.visual_lines.len() {
                                        flash_msg = Some(format!("Line {n} out of range (max {})", doc.visual_lines.len()));
                                    } else {
                                        let text = yank_exact(&markdown, &doc.visual_lines, vl_idx);
                                        if text.is_empty() {
                                            flash_msg = Some(format!("L{n}: no source mapping"));
                                        } else {
                                            let line_count = text.lines().count();
                                            if let Err(e) = terminal::send_osc52(&text) {
                                                debug!("OSC 52 failed: {e}");
                                            }
                                            flash_msg = Some(format!("Yanked L{n} ({line_count} line{})", if line_count > 1 { "s" } else { "" }));
                                            debug!("yank exact L{n}: {} bytes, {line_count} lines", text.len());
                                        }
                                    }
                                    terminal::draw_status_bar(&layout, &state, acc.peek(), flash_msg.as_deref())?;
                                }

                                // ブロックヤンク (Y): 常にブロック全体
                                Some(Action::YankBlockPrompt) => {
                                    flash_msg = Some("Type NY to yank block N".into());
                                    terminal::draw_status_bar(&layout, &state, acc.peek(), flash_msg.as_deref())?;
                                }
                                Some(Action::YankBlock(n)) => {
                                    let vl_idx = (n as usize).saturating_sub(1);
                                    if vl_idx >= doc.visual_lines.len() {
                                        flash_msg = Some(format!("Line {n} out of range (max {})", doc.visual_lines.len()));
                                    } else {
                                        let text = yank_lines(&markdown, &doc.visual_lines, vl_idx, vl_idx);
                                        if text.is_empty() {
                                            flash_msg = Some(format!("L{n}: no source mapping"));
                                        } else {
                                            let line_count = text.lines().count();
                                            if let Err(e) = terminal::send_osc52(&text) {
                                                debug!("OSC 52 failed: {e}");
                                            }
                                            flash_msg = Some(format!("Yanked L{n} block ({line_count} lines)"));
                                            debug!("yank block L{n}: {} bytes, {line_count} lines", text.len());
                                        }
                                    }
                                    terminal::draw_status_bar(&layout, &state, acc.peek(), flash_msg.as_deref())?;
                                }

                                None => {
                                    // Unknown key: reset accumulator
                                    if acc.is_active() {
                                        acc.reset();
                                        terminal::draw_status_bar(&layout, &state, None, None)?;
                                    } else if had_flash {
                                        terminal::draw_status_bar(&layout, &state, None, None)?;
                                    }
                                }
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
                    while let Ok((idx, pngs)) = res_rx.try_recv() {
                        debug!("main: received prefetched tile {idx} (content={}, sidebar={} bytes, pre-redraw)", pngs.content.len(), pngs.sidebar.len());
                        in_flight.remove(&idx);
                        cache.insert(idx, pngs);
                    }
                    self::state::redraw(doc, &mut cache, &mut loaded, &layout, &state, acc.peek(), flash_msg.as_deref())?;
                    self::state::send_prefetch(&req_tx, doc, &cache, &mut in_flight, state.y_offset);
                    cache.evict_distant(
                        (state.y_offset / doc.tile_height_px()) as usize,
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
                debug!("resize: rebuilding tiled document and sidebar");
                let new_winsize = crossterm_terminal::window_size()?;
                layout = state::compute_layout(
                    new_cols,
                    new_rows,
                    new_winsize.width,
                    new_winsize.height,
                );
                terminal::delete_all_images()?;
                // continue 'outer → new tiled_doc + new scope + new worker
            }
        }
    }

    guard.cleanup();
    Ok(())
}
