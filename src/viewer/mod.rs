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
mod mode_command;
mod mode_normal;
mod mode_search;
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

use crate::config::{self, CliOverrides, Config};
use crate::convert::markdown_to_typst_with_map;
use crate::tile::{TilePngs, TiledDocumentCache};
use crate::watch::FileWatcher;
use crate::world::FontCache;

use input::{InputAccumulator, map_command_key, map_key_event, map_search_key};
use mode_command::CommandState;
use mode_search::LastSearch;
use state::{ExitReason, LoadedTiles, ViewState};

/// Viewer mode: normal (tile display), search (picker UI), or command (`:` prompt).
enum ViewerMode {
    Normal,
    Search(mode_search::SearchState),
    Command(CommandState),
}

/// Side-effect descriptors produced by mode handlers.
///
/// Handlers return `Vec<Effect>` which the apply loop in `run()` executes.
/// This separates "what to do" (handler) from "how to do it" (apply loop).
enum Effect {
    ScrollTo(u32),
    MarkDirty,
    Flash(String),
    RedrawStatusBar,
    Yank(String),
    SetMode(ViewerMode),
    SetLastSearch(LastSearch),
    DeletePlacements,
    Exit(ExitReason),
}

/// Run the terminal viewer.
///
/// `md_path` is the Markdown file to display.
/// `config` contains all resolved settings (theme, PPI, viewer params, etc.).
/// `cli_overrides` are preserved across config reloads.
/// `watch` enables automatic reload on file change.
pub fn run(
    md_path: PathBuf,
    mut config: Config,
    cli_overrides: &CliOverrides,
    watch: bool,
) -> anyhow::Result<()> {
    let filename = md_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    terminal::check_tty()?;

    // 2. ターミナルサイズを先に取得してビューポート幅を確定
    let winsize = crossterm_terminal::window_size()
        .map_err(|e| anyhow::anyhow!("failed to get terminal size: {e}"))?;
    let (term_cols, term_rows) = (winsize.columns, winsize.rows);
    let (pixel_w, pixel_h) = (winsize.width, winsize.height);

    if pixel_w == 0 || pixel_h == 0 {
        anyhow::bail!(
            "terminal pixel size {}x{} is zero — Kitty graphics requires non-zero pixel dimensions",
            pixel_w,
            pixel_h
        );
    }

    // 3. Font cache (one-time filesystem scan, shared across rebuilds)
    let font_cache = FontCache::new();

    // 4. raw mode + alternate screen (maintained across rebuilds)
    let mut guard = terminal::RawGuard::enter()?;

    let mut layout = state::compute_layout(
        term_cols,
        term_rows,
        pixel_w,
        pixel_h,
        config.viewer.sidebar_cols,
    );
    let mut y_offset_carry: u32 = 0;
    // Flash message to pass from outer loop into inner loop (e.g. "Config reloaded")
    let mut outer_flash: Option<String> = None;

    // File watcher (optional)
    let watcher = if watch {
        Some(FileWatcher::new(&md_path)?)
    } else {
        None
    };

    // Outer loop: each iteration builds a new TiledDocument (initial + resize + reload)
    'outer: loop {
        // 1. テーマを読み込み (config reload でテーマ名が変わる場合があるのでループ内)
        let theme_path = PathBuf::from(format!("themes/{}.typ", config.theme));
        let theme_text = std::fs::read_to_string(&theme_path)
            .map_err(|e| anyhow::anyhow!("failed to read theme {}: {e}", theme_path.display()))?;

        // 5a. Read markdown (re-read on each iteration for reload support)
        let markdown = std::fs::read_to_string(&md_path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", md_path.display()))?;
        let (content_text, source_map) = markdown_to_typst_with_map(&markdown);

        // 5b. Build TiledDocument (content + sidebar compiled & split)
        info!("building tiled document...");
        let tiled_doc = pipeline::build_tiled_document(&pipeline::PipelineInput {
            theme_text: &theme_text,
            content_text: &content_text,
            md_source: &markdown,
            source_map: &source_map,
            layout: &layout,
            ppi: config.ppi,
            tile_height_min: config.viewer.tile_height,
            fonts: &font_cache,
        })?;

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
        let mut loaded = LoadedTiles::new(config.viewer.evict_distance);

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
            let mut flash_msg: Option<String> = outer_flash.take();
            // Viewer mode: normal (tile display) or search (picker UI)
            let mut mode = ViewerMode::Normal;
            // Persisted search results for n/N navigation
            let mut last_search: Option<LastSearch> = None;

            // Initial redraw + prefetch
            self::state::redraw(
                doc,
                &mut cache,
                &mut loaded,
                &layout,
                &state,
                acc.peek(),
                None,
            )?;
            self::state::send_prefetch(&req_tx, doc, &cache, &mut in_flight, state.y_offset);

            // Inner event loop
            let mut dirty = false;
            let mut last_render = Instant::now();

            loop {
                // Drain prefetch results into cache.
                while let Ok((idx, pngs)) = res_rx.try_recv() {
                    debug!(
                        "main: received prefetched tile {idx} (content={}, sidebar={} bytes)",
                        pngs.content.len(),
                        pngs.sidebar.len()
                    );
                    in_flight.remove(&idx);
                    cache.insert(idx, pngs);
                }

                let idle_timeout = if watcher.is_some() {
                    config.viewer.watch_interval
                } else {
                    Duration::from_secs(86400)
                };
                let timeout = if dirty {
                    config
                        .viewer
                        .frame_budget
                        .saturating_sub(last_render.elapsed())
                } else {
                    idle_timeout
                };

                if event::poll(timeout)? {
                    let ev = event::read()?;
                    debug!("event: {:?}", ev);

                    match ev {
                        Event::Key(key_event) => {
                            let max_y = doc.max_scroll(state.vp_h);

                            let effects = match &mut mode {
                                ViewerMode::Normal => {
                                    let had_flash = flash_msg.is_some();
                                    flash_msg = None;

                                    match map_key_event(key_event, &mut acc) {
                                        Some(action) => {
                                            let mut ctx = mode_normal::NormalCtx {
                                                state: &state,
                                                visual_lines: &doc.visual_lines,
                                                max_scroll: max_y,
                                                scroll_step: config.viewer.scroll_step
                                                    * layout.cell_h as u32,
                                                half_page: (layout.image_rows as u32 / 2).max(1)
                                                    * layout.cell_h as u32,
                                                markdown: &markdown,
                                                last_search: &mut last_search,
                                            };
                                            mode_normal::handle(action, &mut ctx)
                                        }
                                        None => {
                                            if acc.is_active() || had_flash {
                                                acc.reset();
                                                vec![Effect::RedrawStatusBar]
                                            } else {
                                                vec![]
                                            }
                                        }
                                    }
                                }
                                ViewerMode::Search(ss) => match map_search_key(key_event) {
                                    Some(a) => mode_search::handle(
                                        a,
                                        ss,
                                        &markdown,
                                        &doc.visual_lines,
                                        &layout,
                                        max_y,
                                    )?,
                                    None => vec![],
                                },
                                ViewerMode::Command(cs) => match map_command_key(key_event) {
                                    Some(a) => mode_command::handle(a, cs, &layout)?,
                                    None => vec![],
                                },
                            };

                            for effect in effects {
                                match effect {
                                    Effect::ScrollTo(y) => {
                                        state.y_offset = y;
                                        dirty = true;
                                    }
                                    Effect::MarkDirty => {
                                        dirty = true;
                                    }
                                    Effect::Flash(msg) => {
                                        flash_msg = Some(msg);
                                    }
                                    Effect::RedrawStatusBar => {
                                        terminal::draw_status_bar(
                                            &layout,
                                            &state,
                                            acc.peek(),
                                            flash_msg.as_deref(),
                                        )?;
                                    }
                                    Effect::Yank(text) => {
                                        let _ = terminal::send_osc52(&text);
                                    }
                                    Effect::SetMode(m) => {
                                        match &m {
                                            ViewerMode::Search(ss) => {
                                                mode_search::draw_search_screen(
                                                    &layout,
                                                    &ss.query,
                                                    &ss.matches,
                                                    ss.selected,
                                                    ss.scroll_offset,
                                                )?;
                                            }
                                            ViewerMode::Command(cs) => {
                                                terminal::draw_command_bar(&layout, &cs.input)?;
                                            }
                                            ViewerMode::Normal => {
                                                dirty = true;
                                            }
                                        }
                                        mode = m;
                                    }
                                    Effect::SetLastSearch(ls) => {
                                        last_search = Some(ls);
                                    }
                                    Effect::DeletePlacements => {
                                        loaded.delete_placements()?;
                                    }
                                    Effect::Exit(reason) => {
                                        return Ok(reason);
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
                        debug!(
                            "main: received prefetched tile {idx} (content={}, sidebar={} bytes, pre-redraw)",
                            pngs.content.len(),
                            pngs.sidebar.len()
                        );
                        in_flight.remove(&idx);
                        cache.insert(idx, pngs);
                    }
                    self::state::redraw(
                        doc,
                        &mut cache,
                        &mut loaded,
                        &layout,
                        &state,
                        acc.peek(),
                        flash_msg.as_deref(),
                    )?;
                    self::state::send_prefetch(
                        &req_tx,
                        doc,
                        &cache,
                        &mut in_flight,
                        state.y_offset,
                    );
                    cache.evict_distant(
                        (state.y_offset / doc.tile_height_px()) as usize,
                        config.viewer.evict_distance,
                    );
                    dirty = false;
                }
                last_render = Instant::now();

                // Check for file changes (non-blocking)
                if let Some(ref w) = watcher
                    && w.has_changed()
                {
                    return Ok(ExitReason::Reload);
                }
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
                    config.viewer.sidebar_cols,
                );
                terminal::delete_all_images()?;
                // continue 'outer → new tiled_doc + new scope + new worker
            }
            ExitReason::Reload => {
                y_offset_carry = state.y_offset;
                debug!("file changed: reloading document");
                terminal::delete_all_images()?;
                // continue 'outer → re-read file + rebuild document
            }
            ExitReason::ConfigReload => {
                y_offset_carry = state.y_offset;
                debug!("config reload requested");

                match config::reload_config(cli_overrides) {
                    Ok(new_config) => {
                        // Verify theme file exists before committing
                        let new_theme_path =
                            PathBuf::from(format!("themes/{}.typ", new_config.theme));
                        if !new_theme_path.exists() {
                            outer_flash = Some(format!(
                                "Reload failed: theme '{}': file not found",
                                new_config.theme
                            ));
                            debug!(
                                "config reload: theme file {} not found, keeping old config",
                                new_theme_path.display()
                            );
                            // Rebuild with old config
                            terminal::delete_all_images()?;
                            continue 'outer;
                        }

                        // Recalculate layout if sidebar_cols changed
                        if new_config.viewer.sidebar_cols != config.viewer.sidebar_cols {
                            let winsize = crossterm_terminal::window_size()?;
                            layout = state::compute_layout(
                                winsize.columns,
                                winsize.rows,
                                winsize.width,
                                winsize.height,
                                new_config.viewer.sidebar_cols,
                            );
                        }

                        config = new_config;
                        outer_flash = Some("Config reloaded".into());
                    }
                    Err(e) => {
                        outer_flash = Some(format!("Reload failed: {e}"));
                        debug!("config reload failed: {e}");
                        // Rebuild with old config
                    }
                }
                terminal::delete_all_images()?;
                // continue 'outer → rebuild document with new (or old) config
            }
        }
    }

    guard.cleanup();
    Ok(())
}
