use std::time::Instant;

use log::info;
use mlux::convert::markdown_to_typst;
use mlux::render::{compile_document, render_frame_to_png};
use mlux::strip::split_frame;
use mlux::world::{FontCache, MluxWorld};

static THEME: &str = include_str!("../../themes/catppuccin.typ");

fn main() {
    env_logger::init();

    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: reproduce <artifact-file-or-markdown>");
        std::process::exit(1);
    });

    let data = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        std::process::exit(1);
    });

    let markdown = match std::str::from_utf8(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Input is not valid UTF-8: {e}");
            std::process::exit(1);
        }
    };

    let iterations = std::env::var("ITERATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);

    eprintln!("=== Input: {} ({} bytes), {} iteration(s) ===", path, markdown.len(), iterations);

    let font_cache = FontCache::new();

    for i in 0..iterations {
        let iter_start = Instant::now();

        let typst_content = markdown_to_typst(markdown);
        let world = MluxWorld::new(THEME, &typst_content, 660.0, &font_cache);

        let document = match compile_document(&world) {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("Compile error: {e}");
                std::process::exit(1);
            }
        };

        if document.pages.is_empty() {
            info!("iteration {i}: no pages, skipping render");
            comemo::evict(0);
            continue;
        }

        let page = &document.pages[0];
        let strips = split_frame(&page.frame, 500.0);
        for strip in &strips {
            if let Err(e) = render_frame_to_png(strip, &page.fill, 144.0) {
                eprintln!("Render error: {e}");
                std::process::exit(1);
            }
        }

        info!(
            "iteration {}: total {:.1}ms",
            i,
            iter_start.elapsed().as_secs_f64() * 1000.0
        );

        comemo::evict(0);
    }
}
