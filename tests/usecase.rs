//! Usecase orchestration integration tests.
//!
//! These tests call `fork()` internally, which is unsafe in multi-threaded
//! processes. We use `harness = false` to avoid the test runner's thread pool
//! and run each test sequentially in a single thread.

use mlux::pipeline::{BuildParams, FontCache, build_tiled_document};
use mlux::tile::VisibleTiles;
use mlux::usecase::{build_dump, build_renderer, build_renderer_blocking};

const DEFAULT_SIDEBAR_WIDTH_PT: f64 = 40.0;

fn test_fork_render_matches_local() {
    let md = "# Hello\n\nSome **bold** text.\n\n- Item 1\n- Item 2\n";
    let font_cache: &'static FontCache = Box::leak(Box::new(FontCache::new()));

    let params = BuildParams {
        theme_spec: "catppuccin".into(),
        detected_light: false,
        markdown: md.into(),
        base_dir: None,
        width_pt: 400.0,
        sidebar_width_pt: DEFAULT_SIDEBAR_WIDTH_PT,
        tile_height_pt: 500.0,
        ppi: 144.0,
        fonts: font_cache,
        allow_remote_images: false,
    };

    // Local render
    let local_doc = build_tiled_document(&params).unwrap();
    let local_meta = local_doc.metadata();

    // Fork render
    let (fork_meta, mut renderer, mut _child) = build_renderer_blocking(&params, true).unwrap();

    // Metadata should match
    assert_eq!(fork_meta.tile_count, local_meta.tile_count);
    assert_eq!(fork_meta.width_px, local_meta.width_px);
    assert_eq!(fork_meta.sidebar_width_px, local_meta.sidebar_width_px);
    assert_eq!(fork_meta.tile_height_px, local_meta.tile_height_px);
    assert_eq!(fork_meta.total_height_px, local_meta.total_height_px);
    assert_eq!(fork_meta.visual_lines.len(), local_meta.visual_lines.len());

    // Rendered tiles should match
    for i in 0..fork_meta.tile_count {
        let fork_pngs = renderer.render_tile_pair(i).unwrap();
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
    renderer.shutdown();
}

fn test_fork_render_metadata_methods() {
    let md = "# Title\n\nParagraph.\n";
    let font_cache: &'static FontCache = Box::leak(Box::new(FontCache::new()));

    let params = BuildParams {
        theme_spec: "catppuccin".into(),
        detected_light: false,
        markdown: md.into(),
        base_dir: None,
        width_pt: 400.0,
        sidebar_width_pt: DEFAULT_SIDEBAR_WIDTH_PT,
        tile_height_pt: 500.0,
        ppi: 144.0,
        fonts: font_cache,
        allow_remote_images: false,
    };

    let (meta, _renderer, mut _child) = build_renderer_blocking(&params, true).unwrap();

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

fn make_failing_params(font_cache: &'static FontCache) -> BuildParams {
    // Trigger a build error via unknown theme to test child→parent error propagation.
    // The specific error type (theme resolution vs. Typst compilation) doesn't matter —
    // both follow the same Response::Error → IPC path.
    BuildParams {
        theme_spec: "nonexistent-theme-for-test".into(),
        detected_light: false,
        markdown: "# Hello\n".into(),
        base_dir: None,
        width_pt: 400.0,
        sidebar_width_pt: 40.0,
        tile_height_pt: 500.0,
        ppi: 144.0,
        fonts: font_cache,
        allow_remote_images: false,
    }
}

fn test_fork_renderer_build_error_propagated() {
    let font_cache: &'static FontCache = Box::leak(Box::new(FontCache::new()));
    let params = make_failing_params(font_cache);

    let (mut renderer, mut _child) = build_renderer(&params, true).unwrap();
    match renderer.wait_for_meta() {
        Ok(_) => panic!("expected build error, got Ok"),
        Err(err) => {
            let msg = format!("{err:#}");
            assert!(
                msg.contains("child build error"),
                "expected 'child build error' in: {msg}"
            );
        }
    }
}

fn test_fork_dump_build_error_exit_code() {
    let font_cache: &'static FontCache = Box::leak(Box::new(FontCache::new()));
    let params = make_failing_params(font_cache);

    let mut child = build_dump(&params, true).unwrap();
    let code = child.wait().unwrap();
    assert_ne!(code, 0, "fork_dump should exit non-zero on build failure");
}

fn main() {
    eprint!("test usecase::test_fork_render_matches_local ... ");
    test_fork_render_matches_local();
    eprintln!("ok");

    eprint!("test usecase::test_fork_render_metadata_methods ... ");
    test_fork_render_metadata_methods();
    eprintln!("ok");

    eprint!("test usecase::test_fork_renderer_build_error_propagated ... ");
    test_fork_renderer_build_error_propagated();
    eprintln!("ok");

    eprint!("test usecase::test_fork_dump_build_error_exit_code ... ");
    test_fork_dump_build_error_exit_code();
    eprintln!("ok");
}
