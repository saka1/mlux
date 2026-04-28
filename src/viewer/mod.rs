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
mod input_history;
mod keymap;
mod layout;
mod mode_command;
mod mode_grep;
mod mode_inline_search;
mod mode_log;
mod mode_normal;
mod mode_toc;
mod mode_url;
pub mod query;
mod scroll;
mod scroll_animator;
mod scroll_policy;
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
use log::{debug, info, warn};
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::app_context::AppContext;
use crate::frame::TileCache;
use crate::input_source::InputSource;
use crate::watch::FileWatcher;

use display_state::{DisplayState, ForkHandle};
use effect::{Effect, ExitReason, ViewerMode};
use input_history::ScrollDirection;
use keymap::{
    Action, InputAccumulator, map_command_key, map_grep_key, map_inline_search_key, map_key_event,
    map_log_key, map_mouse_event, map_toc_key, map_url_key,
};
use layout::ScrollState;
use query::DocumentQuery;
use scroll::ScrollStrategy;
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

    let mut guard = terminal::RawGuard::enter(app.config.viewer.mouse)?;

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

    // Double-buffer: two fixed ID ranges, toggled on reload.
    // Old-generation images stay visible while new ones compile + upload.
    const GEN_BASES: [u32; 2] = [100, 5000];
    let mut active_gen: usize = 0;
    let mut stale_image_ids: Vec<u32> = Vec::new();

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
                InputSource::File(path) => read_with_retry(path)?,
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
        let file_path = match &session.input {
            InputSource::File(path) => Some(path.to_path_buf()),
            InputSource::Stdin(_) => None,
        };

        // 5b. Build document (content + sidebar compiled & split)
        info!("building tiled document...");
        let layout_copy = session.layout;
        let ppi = app.config.ppi;
        let tile_height = app.config.viewer.tile_height;

        // ChildProcess handle kept alive for the duration of the inner loop.
        // Dropped on reload/resize/quit → sends SIGKILL to child.
        let mut _fork_child: Option<crate::renderer::ChildProcess> = None;

        // Build: fork a child process, compile/render there, communicate via IPC.
        let (meta, renderer) = {
            let layout = &layout_copy;
            let ppi_f = ppi as f64;
            let width_pt = layout.viewport_width_pt(ppi_f);
            let vp_height_pt = layout.viewport_height_pt(ppi_f);
            let unaligned_pt = tile_height.max(vp_height_pt);
            let tile_height_pt = layout.align_tile_height_pt(unaligned_pt, ppi_f);
            debug!(
                "tile height alignment: {unaligned_pt}pt -> {tile_height_pt}pt (cell_h={})",
                layout.cell_h
            );
            let sidebar_width_pt = layout.sidebar_width_pt(ppi_f);

            let params = app.build_params(
                markdown.clone(),
                base_dir.map(|p| p.to_path_buf()),
                file_path.clone(),
                width_pt,
                sidebar_width_pt,
                tile_height_pt,
                true,
            );
            // Fork 1 (image extraction) + Fork 2 (renderer) before any threads.
            // The child starts building immediately; we wait for meta below.
            let (mut renderer, child) =
                crate::renderer::build_renderer(&params, no_sandbox, &session.log_buffer)?;
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
                    // Don't clear screen if old-gen images are still displayed
                    let clear = stale_image_ids.is_empty();
                    terminal::draw_loading_screen(&session.layout, &session.filename, clear)?;
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
                            stale_image_ids.clear(); // resize deletes all images
                            active_gen = 0;
                            continue 'outer;
                        }
                        _ => {}
                    }
                }
            }
        };
        // Merge cached tiles from previous generation
        let merge = tile_cache.merge_generation(&meta.tile_hashes);
        if merge.recovered == merge.total {
            info!("merge: recovered {}/{} tiles", merge.recovered, merge.total);
        } else {
            let not_cached = merge.hash_matched - merge.recovered;
            let changed = merge.total - merge.hash_matched;
            info!(
                "merge: recovered {}/{} tiles ({} changed, {} not yet rendered)",
                merge.recovered, merge.total, changed, not_cached,
            );
        }

        let img_w = meta.width_px;
        let img_h = meta.total_height_px;
        let (vp_w, vp_h) = layout::vp_dims(&session.layout, img_w, img_h);

        // 6. Inner event loop
        let mut vp = Viewport {
            mode: ViewerMode::Normal,
            scroll: ScrollState::new(
                session.scroll_carry.min(meta.max_scroll(vp_h)),
                img_h,
                vp_w,
                vp_h,
                app.config.viewer.scroll_animation,
            ),
            display: DisplayState::new_with_start_id(
                app.config.viewer.evict_distance,
                GEN_BASES[active_gen],
            ),
            flash: session.pending_flash.take(),
            dirty: false,
            last_search: None,
            highlights_visible: true,
            pending_zoom_delta: 0,
        };

        // in_flight: set of tile indices sent to the child but not yet received.
        // Inserted by send_prefetch(), removed on try_recv().
        let mut in_flight: HashSet<usize> = HashSet::new();
        let scroll_strategy = ScrollStrategy::from_mode(app.config.viewer.scroll_mode);
        let mut renderer = renderer;

        let exit: anyhow::Result<(ExitReason, u32)> = (|| -> anyhow::Result<(ExitReason, u32)> {
            // Vim-style number prefix accumulator
            let mut acc = InputAccumulator::new();

            // Initial redraw + prefetch
            let search_spec = if vp.highlights_visible {
                vp.last_search.as_ref().map(|ls| ls.highlight_spec())
            } else {
                None
            };
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

            // Double-buffer: clean up old-generation images now that new tiles are placed
            if !stale_image_ids.is_empty() {
                info!(
                    "double-buffer: deleting {} stale image IDs from old gen",
                    stale_image_ids.len(),
                );
                debug!("double-buffer: stale IDs: {:?}", stale_image_ids);
                terminal::delete_images_by_ids(&stale_image_ids)?;
                stale_image_ids.clear();
            }

            // Inner event loop
            let mut last_render = Instant::now();
            let mut last_tick = Instant::now();

            loop {
                // Advance scroll animation toward the history-derived target
                // once per iteration.  Frame-rate independent: dt is the actual
                // elapsed wall-clock time, clamped to keep tick deltas in a
                // sane range across long idle waits.
                let now = Instant::now();
                let dt = now.duration_since(last_tick).min(Duration::from_millis(64));
                last_tick = now;
                if vp.scroll.tick(dt) {
                    vp.dirty = true;
                }

                let has_live_source = session.watcher.is_some()
                    || (matches!(&session.input, InputSource::Stdin(_)) && !stdin_eof);
                let timeout = if vp.dirty || vp.scroll.is_animating() {
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
                                        let dir = match &a {
                                            Action::ScrollDown(_) | Action::HalfPageDown(_) => {
                                                Some(ScrollDirection::Down)
                                            }
                                            Action::ScrollUp(_) | Action::HalfPageUp(_) => {
                                                Some(ScrollDirection::Up)
                                            }
                                            _ => None,
                                        };
                                        let scroll_step = match dir {
                                            Some(d) => scroll_strategy.step(
                                                app.config.viewer.scroll_step,
                                                session.layout.cell_h as u32,
                                                d,
                                                &vp.scroll.input_history,
                                            ),
                                            None => {
                                                app.config.viewer.scroll_step
                                                    * session.layout.cell_h as u32
                                            }
                                        };

                                        let mut ctx = mode_normal::NormalCtx {
                                            scroll: &vp.scroll,
                                            doc: &doc,
                                            max_scroll: max_y,
                                            scroll_step,
                                            wheel_step: app.config.viewer.wheel_step
                                                * session.layout.cell_h as u32,
                                            half_page: (session.layout.image_rows as u32 / 2)
                                                .max(1)
                                                * session.layout.cell_h as u32,
                                            last_search: &mut vp.last_search,
                                            current_file: session.current_file_path(),
                                            current_scale: app.config.scale,
                                        };
                                        mode_normal::handle(a, &mut ctx)
                                    }
                                    None => vec![],
                                },
                                ViewerMode::Grep(gs) => match map_grep_key(key_event) {
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        mode_grep::handle(a, gs, &doc, visible_count, max_y)
                                    }
                                    None => vec![],
                                },
                                ViewerMode::InlineSearch(is) => {
                                    match map_inline_search_key(key_event) {
                                        Some(a) => mode_inline_search::handle(a, is, &doc, max_y),
                                        None => vec![],
                                    }
                                }
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
                                ViewerMode::Log(ls) => match map_log_key(key_event, ls.search_mode)
                                {
                                    Some(a) => {
                                        let visible_count =
                                            (session.layout.status_row - 1) as usize;
                                        let total_cols = (session.layout.sidebar_cols
                                            + session.layout.image_cols)
                                            as usize;
                                        mode_log::handle(a, ls, visible_count, total_cols)
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
                                if matches!(effect, Effect::ToggleWatch) {
                                    match session.current_file_path() {
                                        Some(path) => {
                                            let path = path.to_path_buf();
                                            if session.watch {
                                                session.watch = false;
                                                session.watcher = None;
                                                vp.flash = Some("watch: off".into());
                                            } else {
                                                match FileWatcher::new(&path) {
                                                    Ok(w) => {
                                                        session.watch = true;
                                                        session.watcher = Some(w);
                                                        vp.flash = Some("watch: on".into());
                                                    }
                                                    Err(e) => {
                                                        vp.flash =
                                                            Some(format!("watch: failed ({e})"));
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            vp.flash =
                                                Some("watch: not available for stdin".into());
                                        }
                                    }
                                    continue;
                                }
                                let (new_vp, render_ops) = vp.apply(effect, &ctx);
                                vp = new_vp;
                                if let Some(reason) =
                                    effect::execute_render_ops(render_ops, &mut vp, &ctx)?
                                {
                                    stale_image_ids = vp.display.all_image_ids();
                                    return Ok((reason, vp.scroll.y_offset));
                                }
                            }
                        }

                        Event::Mouse(me) if app.config.viewer.mouse => {
                            let max_y = meta.max_scroll(vp.scroll.vp_h);
                            let doc = DocumentQuery::new(
                                &markdown,
                                &meta.visual_lines,
                                &meta.content_index,
                                meta.content_offset,
                            );

                            // Wheel input is Normal-mode-only; other modes ignore it.
                            let effects = match &mut vp.mode {
                                ViewerMode::Normal => match map_mouse_event(me) {
                                    Some(a) => {
                                        let mut ctx = mode_normal::NormalCtx {
                                            scroll: &vp.scroll,
                                            doc: &doc,
                                            max_scroll: max_y,
                                            scroll_step: app.config.viewer.scroll_step
                                                * session.layout.cell_h as u32,
                                            wheel_step: app.config.viewer.wheel_step
                                                * session.layout.cell_h as u32,
                                            half_page: (session.layout.image_rows as u32 / 2)
                                                .max(1)
                                                * session.layout.cell_h as u32,
                                            last_search: &mut vp.last_search,
                                            current_file: session.current_file_path(),
                                            current_scale: app.config.scale,
                                        };
                                        mode_normal::handle(a, &mut ctx)
                                    }
                                    None => vec![],
                                },
                                _ => vec![],
                            };

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
                                    effect::execute_render_ops(render_ops, &mut vp, &ctx)?
                                {
                                    stale_image_ids = vp.display.all_image_ids();
                                    return Ok((reason, vp.scroll.y_offset));
                                }
                            }
                        }

                        Event::Resize(new_cols, new_rows) => {
                            stale_image_ids = vp.display.all_image_ids();
                            return Ok((
                                ExitReason::Resize { new_cols, new_rows },
                                vp.scroll.y_offset,
                            ));
                        }

                        _ => {}
                    }
                    continue;
                }

                // Poll timed out → frame budget elapsed without new input. Flush
                // any accumulated Ctrl+wheel zoom delta before redrawing so a
                // burst of wheel notches collapses into a single SetScale rebuild.
                if vp.pending_zoom_delta != 0 {
                    let target =
                        mode_normal::compute_zoom_target(app.config.scale, vp.pending_zoom_delta);
                    vp.pending_zoom_delta = 0;

                    // No-op flush (already at preset edge): clear dirty so we
                    // don't trigger a wasted full redraw_and_prefetch below —
                    // AccumulateZoom set dirty=true to shorten the next poll
                    // timeout, but no tile content actually changed.
                    let no_op = (target - app.config.scale).abs() < 1e-9;
                    if no_op {
                        vp.dirty = false;
                    }

                    let doc = DocumentQuery::new(
                        &markdown,
                        &meta.visual_lines,
                        &meta.content_index,
                        meta.content_offset,
                    );
                    let ctx = ViewContext {
                        layout: &session.layout,
                        acc_value: acc.peek(),
                        filename: &session.filename,
                        jump_stack: &session.jump_stack,
                        doc: &doc,
                        log_buffer: &session.log_buffer,
                    };
                    for effect in mode_normal::zoom_effects(app.config.scale, target) {
                        let (new_vp, render_ops) = vp.apply(effect, &ctx);
                        vp = new_vp;
                        if let Some(reason) = effect::execute_render_ops(render_ops, &mut vp, &ctx)?
                        {
                            stale_image_ids = vp.display.all_image_ids();
                            return Ok((reason, vp.scroll.y_offset));
                        }
                    }
                }

                // poll timeout → frame budget elapsed, execute redraw
                if vp.dirty {
                    let doc = DocumentQuery::new(
                        &markdown,
                        &meta.visual_lines,
                        &meta.content_index,
                        meta.content_offset,
                    );
                    let search_spec = match &vp.mode {
                        ViewerMode::InlineSearch(is) if !is.matches.is_empty() => {
                            Some(is.highlight_spec(&doc))
                        }
                        _ => {
                            if vp.highlights_visible {
                                vp.last_search.as_ref().map(|ls| ls.highlight_spec())
                            } else {
                                None
                            }
                        }
                    };
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
                    info!("file change detected, reloading");
                    stale_image_ids = vp.display.all_image_ids();
                    return Ok((ExitReason::Reload, vp.scroll.y_offset));
                }
            }
        })();
        renderer.shutdown();
        let (exit, scroll_y) = exit?;

        // Apply scale change before tile cache decision, so the next build uses it.
        // Scale change invalidates all tile hashes (theme pt × scale changes Frame
        // tree), so the cache merge below would be a no-op anyway — clear it.
        if let ExitReason::SetScale { new, .. } = &exit {
            app.config.scale = *new;
            tile_cache.clear();
        }

        // Discard cache on navigation (reload/resize/scale keeps it for merge_generation)
        match &exit {
            ExitReason::Reload | ExitReason::Resize { .. } | ExitReason::SetScale { .. } => {}
            _ => {
                tile_cache.clear();
            }
        }

        // Double-buffer: toggle generation on reload/scale, reset on everything else
        if matches!(&exit, ExitReason::Reload | ExitReason::SetScale { .. }) {
            active_gen ^= 1;
            debug!(
                "double-buffer: gen {} → {}, stale {} IDs (base={})",
                active_gen ^ 1,
                active_gen,
                stale_image_ids.len(),
                GEN_BASES[active_gen],
            );
        } else {
            // Resize/Navigate/etc call delete_all_images() → reset to gen 0
            active_gen = 0;
            stale_image_ids.clear();
        }

        if session.handle_exit(exit, scroll_y, app.config.viewer.sidebar_cols)? {
            break 'outer;
        }
    }

    guard.cleanup();
    Ok(())
}

/// Interval between retries when a file is temporarily missing (atomic save).
const RETRY_INTERVAL: Duration = Duration::from_millis(50);
/// Maximum number of retries before giving up.
const RETRY_MAX_ATTEMPTS: u32 = 10;

/// Read a file, retrying on [`std::io::ErrorKind::NotFound`] to tolerate
/// atomic-save editors (vim, emacs, etc.) that briefly remove the file
/// during a write-then-rename sequence.
fn read_with_retry(path: &std::path::Path) -> anyhow::Result<String> {
    let mut attempts = 0;
    loop {
        match std::fs::read_to_string(path) {
            Ok(content) => return Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && attempts < RETRY_MAX_ATTEMPTS => {
                attempts += 1;
                warn!(
                    "file not found (attempt {}/{}), retrying in {}ms: {}",
                    attempts,
                    RETRY_MAX_ATTEMPTS,
                    RETRY_INTERVAL.as_millis(),
                    path.display()
                );
                std::thread::sleep(RETRY_INTERVAL);
            }
            Err(e) => {
                return Err(anyhow::anyhow!("failed to read {}: {e}", path.display()));
            }
        }
    }
}
