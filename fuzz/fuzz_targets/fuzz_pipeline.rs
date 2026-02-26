#![no_main]

use libfuzzer_sys::fuzz_target;
use mlux::convert::markdown_to_typst;
use mlux::render::{compile_document, render_frame_to_png};
use mlux::strip::split_frame;
use mlux::world::MluxWorld;

static THEME: &str = include_str!("../../themes/catppuccin.typ");

fuzz_target!(|data: &[u8]| {
    let Ok(markdown) = std::str::from_utf8(data) else {
        return;
    };

    let typst_content = markdown_to_typst(markdown);
    let world = MluxWorld::new(THEME, &typst_content, 660.0);

    let document = match compile_document(&world) {
        Ok(doc) => doc,
        Err(e) => panic!("compile failed:\n{e}"),
    };

    if document.pages.is_empty() {
        return;
    }
    let page = &document.pages[0];
    let strips = split_frame(&page.frame, 500.0);
    for strip in &strips {
        if let Err(e) = render_frame_to_png(strip, &page.fill, 144.0) {
            panic!("render failed:\n{e}");
        }
    }

    // typst/comemo のメモ化キャッシュをクリア。
    // fuzzer は毎回異なるドキュメントをコンパイルするため、キャッシュが
    // 再利用されずに蓄積し RSS が単調増加 → OOM になる。
    comemo::evict(0);
});
