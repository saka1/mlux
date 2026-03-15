//! Test harness for scenario-based viewer testing without a terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

use crate::highlight::HighlightRect;
use crate::input::InputSource;
use crate::pipeline::{BuildParams, FontCache, build_tiled_document};
use crate::tile::{DocumentMeta, TiledDocument, VisibleTiles};

use super::effect::{Effect, RenderOp, ViewContext, ViewerMode, Viewport};
use super::input::{
    InputAccumulator, map_command_key, map_key_event, map_search_key, map_toc_key, map_url_key,
};
use super::layout::{self, Layout, ScrollState};
use super::query::DocumentQuery;
use super::tiles::LoadedTiles;

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
    input_source: InputSource,
    filename: String,
    render_ops: Vec<RenderOp>,
    scroll_step: u32,
    half_page: u32,
}

#[allow(dead_code)]
impl TestHarness {
    pub fn new(md: &str, cols: u16, rows: u16) -> Self {
        let font_cache = FontCache::new();
        let theme_name = "catppuccin";
        let theme_text = crate::theme::get(theme_name).expect("built-in theme");
        let data_files = crate::theme::data_files(theme_name);

        let pixel_w = cols * CELL_W;
        let pixel_h = rows * CELL_H;
        let layout = layout::compute_layout(cols, rows, pixel_w, pixel_h, SIDEBAR_COLS);

        let width_pt = layout.image_cols as f64 * layout.cell_w as f64 * 72.0 / PPI as f64;
        let sidebar_width_pt =
            layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / PPI as f64;
        let tile_height_pt = 500.0_f64;

        let params = BuildParams {
            theme_name,
            theme_text,
            data_files,
            markdown: md,
            base_dir: None,
            width_pt,
            sidebar_width_pt,
            tile_height_pt,
            ppi: PPI,
            fonts: &font_cache,
            allow_remote_images: false,
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
                img_h: meta.total_height_px,
                vp_w,
                vp_h,
            },
            tiles: LoadedTiles::new(4),
            flash: None,
            dirty: false,
            last_search: None,
        };

        Self {
            viewport,
            layout,
            meta,
            doc,
            markdown: md.to_string(),
            acc: InputAccumulator::new(),
            input_source: InputSource::File(PathBuf::from("test.md")),
            filename: "test.md".to_string(),
            render_ops: Vec::new(),
            scroll_step,
            half_page,
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

        let effects = match &mut self.viewport.mode {
            ViewerMode::Normal => {
                let had_flash = self.viewport.flash.is_some();
                self.viewport.flash = None;
                match map_key_event(key, &mut self.acc) {
                    Some(action) => {
                        let mut ctx = super::mode_normal::NormalCtx {
                            scroll: &self.viewport.scroll,
                            doc: &doc,
                            max_scroll: max_y,
                            scroll_step: self.scroll_step,
                            half_page: self.half_page,
                            last_search: &mut self.viewport.last_search,
                        };
                        super::mode_normal::handle(action, &mut ctx)
                    }
                    None => {
                        if self.acc.is_active() || had_flash {
                            self.acc.reset();
                            vec![Effect::RedrawStatusBar]
                        } else {
                            vec![]
                        }
                    }
                }
            }
            ViewerMode::Search(ss) => match map_search_key(key) {
                Some(a) => {
                    let visible_count = (self.layout.status_row - 1) as usize;
                    super::mode_search::handle(a, ss, &doc, visible_count, max_y)
                }
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
                    super::mode_url::handle(a, up, visible_count)
                }
                None => vec![],
            },
        };

        let ctx = ViewContext {
            layout: &self.layout,
            acc_value: self.acc.peek(),
            input: &self.input_source,
            filename: &self.filename,
            jump_stack: &[],
            doc: &doc,
        };

        let mut ops = Vec::new();
        for effect in effects {
            let mut effect_ops = Vec::new();
            match self.viewport.apply(effect, &ctx, &mut effect_ops) {
                Ok(Some(_exit)) => {
                    ops.extend(effect_ops);
                    break;
                }
                Ok(None) => ops.extend(effect_ops),
                Err(e) => panic!("apply failed: {e}"),
            }
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
        assert!(ops.iter().any(|op| matches!(op, RenderOp::DrawModeScreen)));
        assert!(matches!(h.viewport.mode, ViewerMode::Search(_)));
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
