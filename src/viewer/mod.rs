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

mod display_state;
mod effect;
mod keymap;
mod layout;
mod mode_command;
mod mode_log;
mod mode_normal;
mod mode_search;
mod mode_toc;
mod mode_url;
pub mod query;
mod session;
mod terminal;
mod viewport;

#[cfg(test)]
mod test_harness;
#[cfg(test)]
mod test_highlight;

pub use terminal::{TerminalTheme, detect_terminal_theme};

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal as crossterm_terminal,
};
use log::{debug, info};
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::app_context::{AppContext, AppContextBuilder};
use crate::input_source::InputSource;
use crate::tile_cache::TileCache;
use crate::watch::FileWatcher;

use display_state::{DisplayState, ForkHandle};
use effect::{Effect, ExitReason, ViewerMode};
use keymap::{
    InputAccumulator, map_command_key, map_key_event, map_log_key, map_search_key, map_toc_key,
    map_url_key,
};
use layout::ScrollState;
use query::DocumentQuery;
use session::Session;
use viewport::{ViewContext, Viewport};

/// Fast threshold: if the build completes within this window, skip the loading screen entirely.
const FAST_THRESHOLD: Duration = Duration::from_millis(100);

/// Run the terminal viewer.
///
/// `app` is the shared application context (fonts, config, theme).
/// `input` is the input source (file path or stdin pipe).
/// `watch` enables automatic reload on file change.
/// `no_sandbox` disables Landlock sandbox (fork is always used).
pub fn run(
    mut app: AppContext,
    input: InputSource,
    initial_markdown: String,
    watch: bool,
    no_sandbox: bool,
    log_buffer: crate::log::LogBuffer,
) -> anyhow::Result<()> {
    terminal::check_tty()?;

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
        layout: layout::compute_layout(
            term_cols,
            term_rows,
            pixel_w,
            pixel_h,
            app.config.viewer.sidebar_cols,
        ),
        filename: input.display_name().to_string(),
        input,
        watcher: watcher_init,
        jump_stack: Vec::new(),
        scroll_carry: 0,
        pending_flash: None,
        watch,
        log_buffer,
    };

    // Stdin buffer and EOF flag (stdin mode only)
    let mut stdin_buf = String::new();
    let mut stdin_eof = false;

    // Initial markdown from main.rs prescan (used on first iteration only)
    let mut cached_markdown: Option<String> = Some(initial_markdown);

    // Content-addressed tile cache for merge across rebuilds
    let mut tile_cache = TileCache::new();

    // Outer loop: each iteration builds a new TiledDocument (initial + resize + reload)
    'outer: loop {
        // 5a. Read markdown (re-read on each iteration for reload support)
        // First iteration uses cached_markdown from prescan; subsequent iterations re-read.
        let markdown = if let Some(md) = cached_markdown.take() {
            // First iteration: also seed stdin_buf if stdin mode
            if let InputSource::Stdin(ref reader) = session.input {
                stdin_buf = md.clone();
                stdin_eof |= reader.drain_into(&mut stdin_buf).eof;
                stdin_buf.clone()
            } else {
                md
            }
        } else {
            if let InputSource::Stdin(ref reader) = session.input {
                stdin_eof |= reader.drain_into(&mut stdin_buf).eof;
            }
            match &session.input {
                InputSource::File(path) => std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?,
                InputSource::Stdin(_) => {
                    if stdin_buf.trim().is_empty() {
                        "*(waiting for input...)*".into()
                    } else {
                        stdin_buf.clone()
                    }
                }
            }
        };
        let base_dir = match &session.input {
            InputSource::File(path) => path.parent(),
            InputSource::Stdin(_) => None,
        };

        // 5b. Build document (content + sidebar compiled & split)
        info!("building tiled document...");
        let layout_copy = session.layout;
        let ppi = app.config.ppi;
        let tile_height = app.config.viewer.tile_height;

        // ChildProcess handle kept alive for the duration of the inner loop.
        // Dropped on reload/resize/quit → sends SIGKILL to child.
        let mut _fork_child: Option<crate::usecase::ChildProcess> = None;

        // Build: fork a child process, compile/render there, communicate via IPC.
        let (meta, renderer) = {
            let layout = &layout_copy;
            let viewport_px_w = layout.image_cols as f64 * layout.cell_w as f64;
            let width_pt = viewport_px_w * 72.0 / ppi as f64;
            let vp_height_pt = layout.image_rows as f64 * layout.cell_h as f64 * 72.0 / ppi as f64;
            let tile_height_pt = tile_height.max(vp_height_pt);
            // Align tile height to cell_h boundary so that in the Split case
            // of place_tiles, top_src_h is always a multiple of cell_h,
            // guaranteeing exact 1:1 scaling (no compression artifacts).
            let tile_height_px_raw = (tile_height_pt * ppi as f64 / 72.0).round() as u32;
            let cell_h = layout.cell_h as u32;
            let tile_height_px_aligned = tile_height_px_raw.div_ceil(cell_h) * cell_h;
            debug!(
                "tile height alignment: {tile_height_px_raw}px -> {tile_height_px_aligned}px (cell_h={cell_h})"
            );
            let tile_height_pt = tile_height_px_aligned as f64 * 72.0 / ppi as f64;
            let sidebar_width_pt =
                layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / ppi as f64;

            let params = app.build_params(
                markdown.clone(),
                base_dir.map(|p| p.to_path_buf()),
                width_pt,
                sidebar_width_pt,
                tile_height_pt,
            );
            // Fork 1 (image extraction) + Fork 2 (renderer) before any threads.
            // The child starts building immediately; we wait for meta below.
            let (mut renderer, child) = crate::usecase::build_renderer(&params, no_sandbox)?;
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
                            session.update_layout_for_resize(
                                new_cols,
                                new_rows,
                                app.config.viewer.sidebar_cols,
                            )?;
                            continue 'outer;
                        }
                        _ => {}
                    }
                }
            }
        };
        // Merge cached tiles from previous generation
        let recovered = tile_cache.merge_generation(&meta.tile_hashes);
        info!("merge: recovered {}/{} tiles", recovered, meta.tile_count);

        let img_w = meta.width_px;
        let img_h = meta.total_height_px;
        let (vp_w, vp_h) = layout::vp_dims(&session.layout, img_w, img_h);

        // 6. Inner event loop
        let mut vp = Viewport {
            mode: ViewerMode::Normal,
            scroll: ScrollState {
                y_offset: session.scroll_carry.min(meta.max_scroll(vp_h)),
                img_h,
                vp_w,
                vp_h,
            },
            display: DisplayState::new(app.config.viewer.evict_distance),
            flash: session.pending_flash.take(),
            dirty: false,
            last_search: None,
        };

        // in_flight: set of tile indices sent to the child but not yet received.
        // Inserted by send_prefetch(), removed on try_recv().
        let mut in_flight: HashSet<usize> = HashSet::new();
        let mut renderer = renderer;

        let exit: anyhow::Result<(ExitReason, u32)> = (|| -> anyhow::Result<(ExitReason, u32)> {
            // Vim-style number prefix accumulator
            let mut acc = InputAccumulator::new();

            // Initial redraw + prefetch
            let search_spec = vp.last_search.as_ref().map(|ls| ls.highlight_spec());
            display_state::redraw_and_prefetch(
                &meta,
                &mut tile_cache,
                &mut vp.display,
                &session.layout,
                &vp.scroll,
                &session.filename,
                acc.peek(),
                None,
                search_spec.as_ref(),
                &mut ForkHandle {
                    renderer: &mut renderer,
                    in_flight: &mut in_flight,
                },
            )?;

            // Inner event loop
            let mut last_render = Instant::now();

            loop {
                let has_live_source = session.watcher.is_some()
                    || (matches!(&session.input, InputSource::Stdin(_)) && !stdin_eof);
                let timeout = if vp.dirty {
                    app.config
                        .viewer
                        .frame_budget
                        .saturating_sub(last_render.elapsed())
                } else if has_live_source {
                    app.config.viewer.watch_interval
                } else {
                    Duration::from_secs(86400)
                };

                if event::poll(timeout)? {
                    let ev = event::read()?;
                    debug!("event: {:?}", ev);

                    match ev {
                        Event::Key(key_event) => {
                            let max_y = meta.max_scroll(vp.scroll.vp_h);
                            let doc = DocumentQuery::new(
                                &markdown,
                                &meta.visual_lines,
                                &meta.content_index,
                                meta.content_offset,
                            );

                            // Clear flash on any keypress in Normal mode
                            let had_flash =
                                matches!(vp.mode, ViewerMode::Normal) && vp.flash.take().is_some();

                            let mut effects = match &mut vp.mode {
                                ViewerMode::Normal => match map_key_event(key_event, &mut acc) {
                                    Some(a) => {
                                        let mut ctx = mode_normal::NormalCtx {
                                            scroll: &vp.scroll,
                                            doc: &doc,
                                            max_scroll: max_y,
                                            scroll_step: app.config.viewer.scroll_step
                                                * session.layout.cell_h as u32,
                                            half_page: (session.layout.image_rows as u32 / 2)
                                                .max(1)
                                                * session.layout.cell_h as u32,
                                            last_search: &mut vp.last_search,
                                            current_file: session.current_file_path(),
                                        };
                                        mode_normal::handle(a, &mut ctx)
                                    }
                                    None => vec![],
                                },
                                ViewerMode::Search(ss) => match map_search_key(key_event) {
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        mode_search::handle(a, ss, &doc, visible_count, max_y)
                                    }
                                    None => vec![],
                                },
                                ViewerMode::Command(cs) => match map_command_key(key_event) {
                                    Some(a) => mode_command::handle(a, cs),
                                    None => vec![],
                                },
                                ViewerMode::Toc(ts) => match map_toc_key(key_event) {
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        mode_toc::handle(
                                            a,
                                            ts,
                                            doc.visual_lines,
                                            visible_count,
                                            max_y,
                                        )
                                    }
                                    None => vec![],
                                },
                                ViewerMode::UrlPicker(up) => match map_url_key(key_event) {
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        mode_url::handle(
                                            a,
                                            up,
                                            visible_count,
                                            session.current_file_path(),
                                        )
                                    }
                                    None => vec![],
                                },
                                ViewerMode::Log(ls) => match map_log_key(key_event) {
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        mode_log::handle(a, ls, visible_count)
                                    }
                                    None => vec![],
                                },
                            };

                            // Post: if flash was just cleared, ensure redraw
                            if had_flash && effects.is_empty() {
                                effects.push(Effect::RedrawStatusBar);
                            }

                            let ctx = ViewContext {
                                layout: &session.layout,
                                acc_value: acc.peek(),
                                filename: &session.filename,
                                jump_stack: &session.jump_stack,
                                doc: &doc,
                                log_buffer: &session.log_buffer,
                            };
                            for effect in effects {
                                let (new_vp, render_ops) = vp.apply(effect, &ctx);
                                vp = new_vp;
                                if let Some(reason) =
                                    effect::execute_render_ops(render_ops, &vp, &ctx)?
                                {
                                    return Ok((reason, vp.scroll.y_offset));
                                }
                            }
                        }

                        Event::Resize(new_cols, new_rows) => {
                            return Ok((
                                ExitReason::Resize { new_cols, new_rows },
                                vp.scroll.y_offset,
                            ));
                        }

                        _ => {}
                    }
                    continue;
                }

                // poll timeout → frame budget elapsed, execute redraw
                if vp.dirty {
                    let search_spec = vp.last_search.as_ref().map(|ls| ls.highlight_spec());
                    display_state::redraw_and_prefetch(
                        &meta,
                        &mut tile_cache,
                        &mut vp.display,
                        &session.layout,
                        &vp.scroll,
                        &session.filename,
                        acc.peek(),
                        vp.flash.as_deref(),
                        search_spec.as_ref(),
                        &mut ForkHandle {
                            renderer: &mut renderer,
                            in_flight: &mut in_flight,
                        },
                    )?;
                    tile_cache.evict_distant(
                        (vp.scroll.y_offset / meta.tile_height_px) as usize,
                        app.config.viewer.evict_distance,
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
                    return Ok((ExitReason::Reload, vp.scroll.y_offset));
                }
            }
        })();
        renderer.shutdown();
        let (exit, scroll_y) = exit?;

        // Discard cache on navigation (reload/resize keeps it for merge_generation)
        match &exit {
            ExitReason::Reload | ExitReason::Resize { .. } | ExitReason::ConfigReload => {}
            _ => {
                tile_cache.clear();
            }
        }

        // Handle config reload: rebuild AppContext with new config
        if matches!(&exit, ExitReason::ConfigReload) {
            match crate::config::reload_config(&app.cli_overrides) {
                Ok(new_config) => {
                    // Validate theme before consuming AppContext
                    let resolved = crate::theme::resolve_theme_name(
                        &new_config.theme,
                        app.detected_light,
                        app.has_cjk,
                    );
                    if crate::theme::get(resolved).is_none() {
                        session.pending_flash = Some(format!(
                            "Reload failed: unknown theme '{}'",
                            new_config.theme
                        ));
                        debug!(
                            "config reload: unknown theme '{}', keeping old config",
                            new_config.theme
                        );
                        // continue with old AppContext
                    } else {
                        // Recalculate layout if sidebar_cols changed
                        if new_config.viewer.sidebar_cols != app.config.viewer.sidebar_cols {
                            let winsize = crossterm_terminal::window_size()?;
                            session.layout = layout::compute_layout(
                                winsize.columns,
                                winsize.rows,
                                winsize.width,
                                winsize.height,
                                new_config.viewer.sidebar_cols,
                            );
                        }

                        let new_overrides = app.cli_overrides.clone();
                        app = AppContextBuilder::from_existing(new_config, new_overrides, &app)
                            .build()
                            .expect("theme validated above");
                        session.pending_flash = Some("Config reloaded".into());
                    }
                }
                Err(e) => {
                    session.pending_flash = Some(format!("Reload failed: {e}"));
                    debug!("config reload failed: {e}");
                }
            }
        }

        if session.handle_exit(exit, scroll_y, app.config.viewer.sidebar_cols)? {
            break 'outer;
        }
    }

    guard.cleanup();
    Ok(())
}
