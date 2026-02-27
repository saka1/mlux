#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use mlux::convert::markdown_to_typst;
use mlux::render::{compile_document, render_frame_to_png};
use mlux::tile::split_frame;
use mlux::world::{FontCache, MluxWorld};

static THEME: &str = include_str!("../../themes/catppuccin.typ");
static FONTS: OnceLock<FontCache> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let Ok(markdown) = std::str::from_utf8(data) else {
        return;
    };

    let fonts = FONTS.get_or_init(FontCache::new);
    let typst_content = markdown_to_typst(markdown);
    let world = MluxWorld::new(THEME, &typst_content, 660.0, fonts);

    let document = match compile_document(&world) {
        Ok(doc) => doc,
        Err(e) => panic!("compile failed:\n{e}"),
    };

    if document.pages.is_empty() {
        return;
    }
    let page = &document.pages[0];
    let tiles = split_frame(&page.frame, 500.0);
    for tile in &tiles {
        if let Err(e) = render_frame_to_png(tile, &page.fill, 144.0) {
            panic!("render failed:\n{e}");
        }
    }

    // typst/comemo のメモ化キャッシュをクリア。
    // fuzzer は毎回異なるドキュメントをコンパイルするため、キャッシュが
    // 再利用されずに蓄積し RSS が単調増加 → OOM になる。
    comemo::evict(0);
});
