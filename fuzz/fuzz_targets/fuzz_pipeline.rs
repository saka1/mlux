#![no_main]

use libfuzzer_sys::fuzz_target;
use mlux::convert::markdown_to_typst;
use mlux::render::compile_document;
use mlux::world::MluxWorld;

static THEME: &str = include_str!("../../themes/catppuccin.typ");

fuzz_target!(|data: &[u8]| {
    let Ok(markdown) = std::str::from_utf8(data) else {
        return;
    };

    let typst_content = markdown_to_typst(markdown);
    let world = MluxWorld::new(THEME, &typst_content, 660.0);

    if let Err(e) = compile_document(&world) {
        panic!("compile failed:\n{e}");
    }
});
