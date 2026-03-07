//! Fork render integration tests.
//!
//! These tests call `fork()` internally, which is unsafe in multi-threaded
//! processes. We use `harness = false` to avoid the test runner's thread pool
//! and run each test sequentially in a single thread.

use mlux::fork_render::{Request, Response, spawn_renderer};
use mlux::image::LoadedImages;
use mlux::pipeline::{
    BuildParams, DEFAULT_SIDEBAR_WIDTH_PT, FontCache, build_tiled_document,
    markdown_to_typst_with_map,
};
use mlux::tile::VisibleTiles;

fn load_theme() -> &'static str {
    mlux::theme::get("catppuccin").expect("built-in theme should exist")
}

fn test_fork_render_matches_local() {
    let md = "# Hello\n\nSome **bold** text.\n\n- Item 1\n- Item 2\n";
    let theme_text = load_theme();
    let (content_text, source_map) = markdown_to_typst_with_map(md, None);
    let font_cache = FontCache::new();

    let params = BuildParams {
        theme_text,
        data_files: mlux::theme::data_files("catppuccin"),
        content_text: &content_text,
        md_source: md,
        source_map: &source_map,
        width_pt: 400.0,
        sidebar_width_pt: DEFAULT_SIDEBAR_WIDTH_PT,
        tile_height_pt: 500.0,
        ppi: 144.0,
        fonts: &font_cache,
        image_files: LoadedImages::default(),
    };

    // Local render
    let local_doc = build_tiled_document(&params).unwrap();
    let local_meta = local_doc.metadata();

    // Fork render
    let (fork_meta, mut tx, mut rx, mut _child) = spawn_renderer(&params, None, true).unwrap();

    // Metadata should match
    assert_eq!(fork_meta.tile_count, local_meta.tile_count);
    assert_eq!(fork_meta.width_px, local_meta.width_px);
    assert_eq!(fork_meta.sidebar_width_px, local_meta.sidebar_width_px);
    assert_eq!(fork_meta.tile_height_px, local_meta.tile_height_px);
    assert_eq!(fork_meta.total_height_px, local_meta.total_height_px);
    assert_eq!(fork_meta.visual_lines.len(), local_meta.visual_lines.len());

    // Rendered tiles should match
    for i in 0..fork_meta.tile_count {
        tx.send(&Request::RenderTile(i)).unwrap();
        let fork_pngs = match rx.recv().unwrap() {
            Response::Tile(pngs) => pngs,
            other => panic!("expected Tile, got {:?}", std::mem::discriminant(&other)),
        };
        let local_pngs = local_doc.render_tile_pair(i).unwrap();
        assert_eq!(
            fork_pngs.content, local_pngs.content,
            "content tile {i} should match"
        );
        assert_eq!(
            fork_pngs.sidebar, local_pngs.sidebar,
            "sidebar tile {i} should match"
        );
    }
    tx.send(&Request::Shutdown).unwrap();
}

fn test_fork_render_metadata_methods() {
    let md = "# Title\n\nParagraph.\n";
    let theme_text = load_theme();
    let (content_text, source_map) = markdown_to_typst_with_map(md, None);
    let font_cache = FontCache::new();

    let params = BuildParams {
        theme_text,
        data_files: mlux::theme::data_files("catppuccin"),
        content_text: &content_text,
        md_source: md,
        source_map: &source_map,
        width_pt: 400.0,
        sidebar_width_pt: DEFAULT_SIDEBAR_WIDTH_PT,
        tile_height_pt: 500.0,
        ppi: 144.0,
        fonts: &font_cache,
        image_files: LoadedImages::default(),
    };

    let (meta, _tx, _rx, mut _child) = spawn_renderer(&params, None, true).unwrap();

    // DocumentMeta methods should work
    assert!(meta.tile_count > 0);
    assert!(meta.total_height_px > 0);
    assert_eq!(meta.max_scroll(meta.total_height_px), 0);
    assert!(meta.max_scroll(100) > 0 || meta.total_height_px <= 100);

    let visible = meta.visible_tiles(0, 100);
    match visible {
        VisibleTiles::Single { idx, src_y, .. } => {
            assert_eq!(idx, 0);
            assert_eq!(src_y, 0);
        }
        VisibleTiles::Split { top_idx, .. } => {
            assert_eq!(top_idx, 0);
        }
    }
}

fn main() {
    eprint!("test fork_render::test_fork_render_matches_local ... ");
    test_fork_render_matches_local();
    eprintln!("ok");

    eprint!("test fork_render::test_fork_render_metadata_methods ... ");
    test_fork_render_metadata_methods();
    eprintln!("ok");
}
