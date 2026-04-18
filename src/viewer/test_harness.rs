//! Test harness for scenario-based viewer testing without a terminal.

use crate::compile::FontCache;
use crate::frame::{DocumentMeta, HighlightRect, TiledDocument, VisibleTiles};
use crate::pipeline::{BuildParams, build_tiled_document};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::display_state::DisplayState;
use super::effect::{Effect, RenderOp, ViewerMode};
use super::keymap::{
    InputAccumulator, map_command_key, map_grep_key, map_key_event, map_log_key, map_toc_key,
    map_url_key,
};
use super::layout::{self, Layout, ScrollState};
use super::query::DocumentQuery;
use super::viewport::{ViewContext, Viewport};

const CELL_W: u16 = 10;
const CELL_H: u16 = 20;
const PPI: f32 = 144.0;
const SCROLL_STEP: u32 = 3;
const SIDEBAR_COLS: u16 = 6;

#[allow(dead_code)]
pub(super) struct TestHarness {
    pub viewport: Viewport,
    layout: Layout,
    meta: DocumentMeta,
    doc: TiledDocument,
    markdown: String,
    acc: InputAccumulator,
    filename: String,
    render_ops: Vec<RenderOp>,
    scroll_step: u32,
    half_page: u32,
    log_buffer: crate::log::LogBuffer,
}

#[allow(dead_code)]
impl TestHarness {
    pub fn new(md: &str, cols: u16, rows: u16) -> Self {
        let font_cache: &'static FontCache = Box::leak(Box::new(FontCache::new()));

        let pixel_w = cols * CELL_W;
        let pixel_h = rows * CELL_H;
        let layout = layout::compute_layout(cols, rows, pixel_w, pixel_h, SIDEBAR_COLS);

        let width_pt = layout.viewport_width_pt(PPI as f64);
        let sidebar_width_pt = layout.sidebar_width_pt(PPI as f64);
        let tile_height_pt = 500.0_f64;

        let params = BuildParams {
            theme_spec: "catppuccin".into(),
            detected_light: false,
            markdown: md.into(),
            base_dir: None,
            file_path: None,
            width_pt,
            sidebar_width_pt,
            tile_height_pt,
            ppi: PPI,
            scale: 1.0,
            fonts: font_cache,
            allow_remote_images: false,
            fast_png: true,
        };

        let doc = build_tiled_document(&params).expect("test document build");
        let meta = doc.metadata();

        let (vp_w, vp_h) = layout::vp_dims(&layout, meta.width_px, meta.total_height_px);
        let scroll_step = SCROLL_STEP * layout.cell_h as u32;
        let half_page = (layout.image_rows as u32 / 2).max(1) * layout.cell_h as u32;

        let viewport = Viewport {
            mode: ViewerMode::Normal,
            scroll: ScrollState {
                y_offset: 0,
                current_y: 0.0,
                target_y: 0,
                img_h: meta.total_height_px,
                vp_w,
                vp_h,
            },
            display: DisplayState::new(4),
            flash: None,
            dirty: false,
            last_search: None,
            highlights_visible: true,
        };

        Self {
            viewport,
            layout,
            meta,
            doc,
            markdown: md.to_string(),
            acc: InputAccumulator::new(),
            filename: "test.md".to_string(),
            render_ops: Vec::new(),
            scroll_step,
            half_page,
            log_buffer: crate::log::LogBuffer::new(16),
        }
    }

    pub fn feed_key(&mut self, key: KeyEvent) -> Vec<RenderOp> {
        let max_y = self.meta.max_scroll(self.viewport.scroll.vp_h);
        let doc = DocumentQuery::new(
            &self.markdown,
            &self.meta.visual_lines,
            &self.meta.content_index,
            self.meta.content_offset,
        );

        // Clear flash on any keypress in Normal mode
        let had_flash = matches!(self.viewport.mode, ViewerMode::Normal)
            && self.viewport.flash.take().is_some();

        let mut effects = match &mut self.viewport.mode {
            ViewerMode::Normal => match map_key_event(key, &mut self.acc) {
                Some(action) => {
                    let mut ctx = super::mode_normal::NormalCtx {
                        scroll: &self.viewport.scroll,
                        doc: &doc,
                        max_scroll: max_y,
                        scroll_step: self.scroll_step,
                        half_page: self.half_page,
                        last_search: &mut self.viewport.last_search,
                        current_file: None,
                        current_scale: 1.0,
                    };
                    super::mode_normal::handle(action, &mut ctx)
                }
                None => vec![],
            },
            ViewerMode::Grep(gs) => match map_grep_key(key) {
                Some(a) => {
                    let visible_count = (self.layout.status_row - 1) as usize;
                    super::mode_grep::handle(a, gs, &doc, visible_count, max_y)
                }
                None => vec![],
            },
            ViewerMode::InlineSearch(is) => match super::keymap::map_inline_search_key(key) {
                Some(a) => super::mode_inline_search::handle(a, is, &doc, max_y),
                None => vec![],
            },
            ViewerMode::Command(cs) => match map_command_key(key) {
                Some(a) => super::mode_command::handle(a, cs),
                None => vec![],
            },
            ViewerMode::Toc(ts) => match map_toc_key(key) {
                Some(a) => {
                    let visible_count = (self.layout.status_row - 1) as usize;
                    super::mode_toc::handle(a, ts, doc.visual_lines, visible_count, max_y)
                }
                None => vec![],
            },
            ViewerMode::UrlPicker(up) => match map_url_key(key) {
                Some(a) => {
                    let visible_count = (self.layout.status_row - 1) as usize;
                    super::mode_url::handle(a, up, visible_count, None)
                }
                None => vec![],
            },
            ViewerMode::Log(ls) => match map_log_key(key, ls.search_mode) {
                Some(a) => {
                    let visible_count = (self.layout.status_row - 1) as usize;
                    let total_cols = (self.layout.sidebar_cols + self.layout.image_cols) as usize;
                    super::mode_log::handle(a, ls, visible_count, total_cols)
                }
                None => vec![],
            },
        };

        // Post: if flash was just cleared, ensure redraw
        if had_flash && effects.is_empty() {
            effects.push(Effect::RedrawStatusBar);
        }

        let ctx = ViewContext {
            layout: &self.layout,
            acc_value: self.acc.peek(),
            filename: &self.filename,
            jump_stack: &[],
            doc: &doc,
            log_buffer: &self.log_buffer,
        };

        let mut ops = Vec::new();
        for effect in effects {
            let vp = std::mem::take(&mut self.viewport);
            let (new_vp, effect_ops) = vp.apply(effect, &ctx);
            self.viewport = new_vp;
            let has_exit = effect_ops.iter().any(|op| matches!(op, RenderOp::Exit(_)));
            ops.extend(effect_ops);
            if has_exit {
                break;
            }
        }

        // Drain the scroll animation: tests observe the final resting position
        // rather than a partially-interpolated frame. The real loop ticks over
        // many frames; the harness collapses that to a single step.
        if self.viewport.scroll.is_animating() {
            self.viewport
                .scroll
                .tick(std::time::Duration::from_secs(10));
        }

        self.render_ops = ops.clone();
        ops
    }

    pub fn feed_keys(&mut self, keys: &str) {
        let mut all_ops = Vec::new();
        for key in parse_keys(keys) {
            all_ops.extend(self.feed_key(key));
        }
        self.render_ops = all_ops;
    }

    pub fn scroll_y(&self) -> u32 {
        self.viewport.scroll.y_offset
    }

    pub fn is_dirty(&self) -> bool {
        self.viewport.dirty
    }

    pub fn flash(&self) -> Option<&str> {
        self.viewport.flash.as_deref()
    }

    pub fn last_yanked(&self) -> Option<&str> {
        self.render_ops.iter().rev().find_map(|op| match op {
            RenderOp::CopyToClipboard(s) => Some(s.as_str()),
            _ => None,
        })
    }

    pub fn visible_tiles(&self) -> VisibleTiles {
        self.meta
            .visible_tiles(self.viewport.scroll.y_offset, self.viewport.scroll.vp_h)
    }

    pub fn highlight_rects(&self, tile_idx: usize) -> Vec<HighlightRect> {
        let Some(ls) = &self.viewport.last_search else {
            return Vec::new();
        };
        let spec = ls.highlight_spec();
        self.doc.find_tile_highlight_rects(tile_idx, &spec)
    }

    pub fn render_ops(&self) -> &[RenderOp] {
        &self.render_ops
    }
}

fn parse_keys(input: &str) -> Vec<KeyEvent> {
    input
        .chars()
        .map(|c| match c {
            '\n' => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            '\x1b' => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            c if c.is_ascii_uppercase() => KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT),
            c => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_constructs_without_panic() {
        let _h = TestHarness::new("# Hello\n\nWorld\n", 80, 24);
    }

    #[test]
    fn yank_heading() {
        let mut h = TestHarness::new("# Hello\n\nWorld\n", 80, 24);
        h.feed_keys("1y");
        assert_eq!(h.last_yanked(), Some("# Hello"));
    }

    #[test]
    fn entering_search_emits_draw() {
        let mut h = TestHarness::new("# Hello\n", 80, 24);
        let ops = h.feed_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(ops.iter().any(|op| matches!(op, RenderOp::DrawStatusBar)));
        assert!(matches!(h.viewport.mode, ViewerMode::InlineSearch(_)));
    }

    #[test]
    fn search_sets_last_search() {
        let mut h = TestHarness::new("# Title\n\nfoo bar foo\n", 80, 24);
        h.feed_keys("/foo\n");
        assert!(matches!(h.viewport.mode, ViewerMode::Normal));
        assert!(h.viewport.last_search.is_some());
    }

    #[test]
    fn jump_to_bottom_scrolls() {
        let long_md = format!("# Title\n\n{}", "line\n".repeat(100));
        let mut h = TestHarness::new(&long_md, 80, 24);
        h.feed_keys("G");
        assert!(h.scroll_y() > 0);
        assert!(h.is_dirty());
    }

    #[test]
    fn flash_cleared_on_next_key() {
        let mut h = TestHarness::new("# Hello\n", 80, 24);
        h.feed_keys("y"); // YankExactPrompt → flash
        assert!(h.flash().is_some());
        h.feed_keys("j"); // next key clears flash
        assert!(h.flash().is_none());
    }
}
