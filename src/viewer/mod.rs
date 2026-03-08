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

mod effect;
mod input;
mod mode_command;
mod mode_normal;
mod mode_search;
mod mode_url;
mod state;
mod terminal;

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal as crossterm_terminal,
};
use log::{debug, info};
use std::collections::HashSet;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{CliOverrides, Config};
use crate::input::InputSource;
use crate::pipeline::FontCache;
use crate::tile::{TilePngs, TiledDocumentCache};
use crate::watch::FileWatcher;

use effect::{Effect, Session, ViewContext, ViewerMode, Viewport};
use input::{InputAccumulator, map_command_key, map_key_event, map_search_key, map_url_key};
use state::{ExitReason, LoadedTiles, PrefetchChannels, ViewState};

/// Fast threshold: if the build completes within this window, skip the loading screen entirely.
const FAST_THRESHOLD: Duration = Duration::from_millis(100);

/// Run the terminal viewer.
///
/// `input` is the input source (file path or stdin pipe).
/// `config` contains all resolved settings (theme, PPI, viewer params, etc.).
/// `cli_overrides` are preserved across config reloads.
/// `watch` enables automatic reload on file change.
/// `no_sandbox` disables Landlock sandbox (fork is always used).
pub fn run(
    input: InputSource,
    config: Config,
    cli_overrides: &CliOverrides,
    watch: bool,
    no_sandbox: bool,
) -> anyhow::Result<()> {
    terminal::check_tty()?;

    // 2. Get terminal size first to determine the viewport width
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
    //
    // `Box::leak` turns the heap allocation into a `&'static` reference.
    // This is sound here because:
    //   - `font_cache` is intentionally immortal: it must outlive every build
    //     thread, the inner event loop, and all outer-loop rebuilds.
    //   - The process exits immediately after `run()` returns, so the leak
    //     has no practical consequence (the OS reclaims the memory anyway).
    //   - The alternative — `Arc<FontCache>` — would require cloning the Arc
    //     into every `thread::spawn` / `thread::scope` closure.  Since the
    //     lifetime is truly "until process exit", `&'static` is both simpler
    //     and more honest about the intent than reference-counting.
    let font_cache: &'static FontCache = Box::leak(Box::new(FontCache::new()));

    // 4. raw mode + alternate screen (maintained across rebuilds)
    let mut guard = terminal::RawGuard::enter()?;

    // Session: persistent state across document rebuilds
    let watcher_init = if watch {
        match &input {
            InputSource::File(path) => Some(FileWatcher::new(path)?),
            InputSource::Stdin(_) => None,
        }
    } else {
        None
    };
    let mut session = Session {
        layout: state::compute_layout(
            term_cols,
            term_rows,
            pixel_w,
            pixel_h,
            config.viewer.sidebar_cols,
        ),
        filename: input.display_name().to_string(),
        config,
        cli_overrides: cli_overrides.clone(),
        input,
        watcher: watcher_init,
        jump_stack: Vec::new(),
        scroll_carry: 0,
        pending_flash: None,
        watch,
    };

    // Stdin buffer and EOF flag (stdin mode only)
    let mut stdin_buf = String::new();
    let mut stdin_eof = false;

    // Outer loop: each iteration builds a new TiledDocument (initial + resize + reload)
    'outer: loop {
        // 1. Load theme (inside the loop because config reload may change the theme name)
        let theme_text = crate::theme::get(&session.config.theme)
            .ok_or_else(|| anyhow::anyhow!("unknown theme '{}'", session.config.theme))?;

        // 5a. Read markdown (re-read on each iteration for reload support)
        // For stdin mode, drain any available data first
        if let InputSource::Stdin(ref reader) = session.input {
            stdin_eof |= reader.drain_into(&mut stdin_buf).eof;
        }
        let markdown = match &session.input {
            InputSource::File(path) => std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?,
            InputSource::Stdin(_) => {
                if stdin_buf.trim().is_empty() {
                    "*(waiting for input...)*".into()
                } else {
                    stdin_buf.clone()
                }
            }
        };
        // Load images
        let base_dir = match &session.input {
            InputSource::File(path) => path.parent(),
            InputSource::Stdin(_) => None,
        };
        let image_paths = crate::pipeline::extract_image_paths(&markdown);
        let (image_files, image_errors) = crate::image::load_images(&image_paths, base_dir);
        for err in &image_errors {
            eprintln!("warning: {err}");
        }
        let loaded_set = image_files.key_set();

        let (content_text, source_map) =
            crate::pipeline::markdown_to_typst(&markdown, Some(&loaded_set));

        // 5b. Build document (content + sidebar compiled & split)
        //
        // On unix with sandbox enabled: fork a child process, compile/render
        // in the sandboxed child, communicate via IPC.
        // Otherwise: build directly in-process on a background thread.
        //
        // Both paths use build_async_with_threshold for loading UI.
        info!("building tiled document...");
        let markdown_clone = markdown.clone(); // markdown also needed by inner loop
        let layout_copy = session.layout;
        let ppi = session.config.ppi;
        let tile_height = session.config.viewer.tile_height;
        let data_files = crate::theme::data_files(&session.config.theme);

        // ChildProcess handle kept alive for the duration of the inner loop.
        // Dropped on reload/resize/quit → sends SIGKILL to child.
        let mut _fork_child: Option<crate::fork_render::ChildProcess> = None;

        // Build: fork a child process, compile/render there, communicate via IPC.
        let (meta, renderer) = {
            let layout = &layout_copy;
            let viewport_px_w = layout.image_cols as f64 * layout.cell_w as f64;
            let width_pt = viewport_px_w * 72.0 / ppi as f64;
            let vp_height_pt = layout.image_rows as f64 * layout.cell_h as f64 * 72.0 / ppi as f64;
            let tile_height_pt = tile_height.max(vp_height_pt);
            let sidebar_width_pt =
                layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / ppi as f64;

            let params = crate::pipeline::BuildParams {
                theme_text,
                data_files,
                content_text: &content_text,
                md_source: &markdown_clone,
                source_map: &source_map,
                width_pt,
                sidebar_width_pt,
                tile_height_pt,
                ppi,
                fonts: font_cache,
                image_files,
            };
            let read_base = match &session.input {
                InputSource::File(p) => p.parent().and_then(|d| d.canonicalize().ok()),
                InputSource::Stdin(_) => None,
            };
            // Fork before any threads (fork safety).
            // The child starts building immediately; we wait for meta below.
            let (mut renderer, child) =
                crate::fork_render::fork_renderer(&params, read_base.as_deref(), no_sandbox)?;
            _fork_child = Some(child);

            // Wait for metadata from child, polling for quit/resize events.
            // Phase 1: poll without loading screen (fast builds complete here).
            // Phase 2: show loading screen after FAST_THRESHOLD.
            let fast_deadline = Instant::now() + FAST_THRESHOLD;
            let mut loading_shown = false;
            loop {
                if renderer.has_pending_data() {
                    let meta = renderer.wait_for_meta()?;
                    info!("fork build complete: {} tiles", meta.tile_count);
                    break (meta, renderer);
                }
                if !loading_shown && Instant::now() >= fast_deadline {
                    terminal::draw_loading_screen(&session.layout, &session.filename)?;
                    loading_shown = true;
                }
                if event::poll(Duration::from_millis(16))? {
                    match event::read()? {
                        Event::Key(k)
                            if k.code == KeyCode::Char('q')
                                || k.code == KeyCode::Esc
                                || (k.code == KeyCode::Char('c')
                                    && k.modifiers.contains(KeyModifiers::CONTROL)) =>
                        {
                            break 'outer;
                        }
                        Event::Resize(new_cols, new_rows) => {
                            session.update_layout_for_resize(new_cols, new_rows)?;
                            continue 'outer;
                        }
                        _ => {}
                    }
                }
            }
        };
        let img_w = meta.width_px;
        let img_h = meta.total_height_px;
        let (vp_w, vp_h) = state::vp_dims(&session.layout, img_w, img_h);

        let mut cache = TiledDocumentCache::new();

        // 6. thread::scope — prefetch worker + inner event loop
        let mut vp = Viewport {
            mode: ViewerMode::Normal,
            view: ViewState {
                y_offset: session.scroll_carry.min(meta.max_scroll(vp_h)),
                img_h,
                vp_w,
                vp_h,
                filename: session.filename.clone(),
            },
            tiles: LoadedTiles::new(session.config.viewer.evict_distance),
            flash: session.pending_flash.take(),
            dirty: false,
            last_search: None,
        };

        let exit = thread::scope(|s| -> anyhow::Result<ExitReason> {
            let (req_tx, req_rx) = mpsc::channel::<usize>();
            let (res_tx, res_rx) = mpsc::channel::<(usize, TilePngs)>();

            // Prefetch worker: FIFO — process each request in order.
            //
            // We do NOT drain-to-latest: send_prefetch() enqueues multiple
            // independent requests [current+1, current+2, current-1], so
            // keeping only the last would skip nearby tiles and force
            // synchronous rendering on the main thread.
            //
            // Results flow back to main via res_tx/res_rx channel.
            // The main thread drains res_rx.try_recv() into the cache.
            let mut renderer = renderer;
            s.spawn(move || {
                debug!("prefetch worker: started");
                while let Ok(idx) = req_rx.recv() {
                    debug!("prefetch worker: rendering tile {idx}");
                    let render_start = Instant::now();
                    match renderer.render_tile_pair(idx) {
                        Ok(pngs) => {
                            debug!("prefetch worker: tile {idx} done in {:.1}ms (content={}, sidebar={} bytes)", render_start.elapsed().as_secs_f64() * 1000.0, pngs.content.len(), pngs.sidebar.len());
                            let _ = res_tx.send((idx, pngs));
                        }
                        Err(e) => {
                            log::error!("prefetch worker: tile {idx} failed: {e}");
                        }
                    }
                }
                renderer.shutdown();
                debug!("prefetch worker: exiting");
            });

            // in_flight: set of tile indices sent to the worker but not yet received.
            // Owned exclusively by the main thread (worker never accesses it).
            // Inserted by send_prefetch(), removed on res_rx.try_recv().
            // Checked together with cache to prevent duplicate renders.
            let mut in_flight: HashSet<usize> = HashSet::new();

            // Vim-style number prefix accumulator
            let mut acc = InputAccumulator::new();

            // Initial redraw + prefetch
            state::redraw(
                &meta,
                &mut cache,
                &mut vp.tiles,
                &session.layout,
                &vp.view,
                acc.peek(),
                None,
                &mut PrefetchChannels {
                    req_tx: &req_tx,
                    res_rx: &res_rx,
                    in_flight: &mut in_flight,
                },
            )?;
            state::send_prefetch(&req_tx, &meta, &cache, &mut in_flight, vp.view.y_offset);

            // Inner event loop
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

                let has_live_source = session.watcher.is_some()
                    || (matches!(&session.input, InputSource::Stdin(_)) && !stdin_eof);
                let idle_timeout = if has_live_source {
                    session.config.viewer.watch_interval
                } else {
                    Duration::from_secs(86400)
                };
                let timeout = if vp.dirty {
                    session
                        .config
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
                            let max_y = meta.max_scroll(vp.view.vp_h);

                            let effects = match &mut vp.mode {
                                ViewerMode::Normal => {
                                    let had_flash = vp.flash.is_some();
                                    vp.flash = None;

                                    match map_key_event(key_event, &mut acc) {
                                        Some(action) => {
                                            let mut ctx = mode_normal::NormalCtx {
                                                state: &vp.view,
                                                visual_lines: &meta.visual_lines,
                                                max_scroll: max_y,
                                                scroll_step: session.config.viewer.scroll_step
                                                    * session.layout.cell_h as u32,
                                                half_page: (session.layout.image_rows as u32 / 2)
                                                    .max(1)
                                                    * session.layout.cell_h as u32,
                                                markdown: &markdown,
                                                last_search: &mut vp.last_search,
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
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        mode_search::handle(
                                            a,
                                            ss,
                                            &markdown,
                                            &meta.visual_lines,
                                            visible_count,
                                            max_y,
                                        )
                                    }
                                    None => vec![],
                                },
                                ViewerMode::Command(cs) => match map_command_key(key_event) {
                                    Some(a) => mode_command::handle(a, cs),
                                    None => vec![],
                                },
                                ViewerMode::UrlPicker(up) => match map_url_key(key_event) {
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        mode_url::handle(a, up, visible_count)
                                    }
                                    None => vec![],
                                },
                            };

                            let ctx = ViewContext {
                                layout: &session.layout,
                                acc_value: acc.peek(),
                                input: &session.input,
                                jump_stack: &session.jump_stack,
                                markdown: &markdown,
                                visual_lines: &meta.visual_lines,
                            };
                            for effect in effects {
                                if let Some(reason) = vp.apply(effect, &ctx)? {
                                    return Ok(reason);
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
                if vp.dirty {
                    // Extra drain just before redraw: collect results the worker
                    // completed while event::poll() was blocking, avoiding
                    // synchronous rendering during redraw.
                    while let Ok((idx, pngs)) = res_rx.try_recv() {
                        debug!(
                            "main: received prefetched tile {idx} (content={}, sidebar={} bytes, pre-redraw)",
                            pngs.content.len(),
                            pngs.sidebar.len()
                        );
                        in_flight.remove(&idx);
                        cache.insert(idx, pngs);
                    }
                    state::redraw(
                        &meta,
                        &mut cache,
                        &mut vp.tiles,
                        &session.layout,
                        &vp.view,
                        acc.peek(),
                        vp.flash.as_deref(),
                        &mut PrefetchChannels {
                            req_tx: &req_tx,
                            res_rx: &res_rx,
                            in_flight: &mut in_flight,
                        },
                    )?;
                    state::send_prefetch(&req_tx, &meta, &cache, &mut in_flight, vp.view.y_offset);
                    cache.evict_distant(
                        (vp.view.y_offset / meta.tile_height_px) as usize,
                        session.config.viewer.evict_distance,
                    );
                    vp.dirty = false;
                }
                last_render = Instant::now();

                // Check for content changes (file watcher or stdin new data)
                let content_changed = match &session.input {
                    InputSource::File(_) => {
                        session.watcher.as_ref().is_some_and(|w| w.has_changed())
                    }
                    InputSource::Stdin(reader) => {
                        let result = reader.drain_into(&mut stdin_buf);
                        stdin_eof |= result.eof;
                        result.got_data
                    }
                };
                if content_changed {
                    return Ok(ExitReason::Reload);
                }
            }
            // req_tx dropped here → worker recv() gets Err → worker exits → scope joins
        })?;

        if session.handle_exit(exit, vp.view.y_offset)? {
            break 'outer;
        }
    }

    guard.cleanup();
    Ok(())
}
