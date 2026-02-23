use std::fs;

use mlux::convert::markdown_to_typst;
use mlux::render::render_to_png;
use mlux::world::MluxWorld;

fn load_theme() -> String {
    fs::read_to_string("themes/catppuccin.typ").expect("theme file should exist")
}

#[test]
fn test_paragraph_ja_renders() {
    let markdown =
        fs::read_to_string("tests/fixtures/01_paragraph_ja.md").expect("fixture should exist");
    let theme = load_theme();
    let content = markdown_to_typst(&markdown);
    let world = MluxWorld::new(&theme, &content, 800.0);
    let png_data = render_to_png(&world, 144.0).expect("rendering should succeed");

    // Check PNG magic bytes
    assert_eq!(&png_data[..8], b"\x89PNG\r\n\x1a\n", "output should be valid PNG");

    // Check minimum size (should be a meaningful image, not just a tiny dot)
    assert!(
        png_data.len() > 1000,
        "PNG should be larger than 1KB, got {} bytes",
        png_data.len()
    );
}

#[test]
fn test_empty_input() {
    let theme = load_theme();
    let content = markdown_to_typst("");
    let world = MluxWorld::new(&theme, &content, 800.0);
    let png_data = render_to_png(&world, 144.0).expect("empty input should still render");

    assert_eq!(&png_data[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn test_full_document_renders() {
    let markdown =
        fs::read_to_string("tests/fixtures/07_full_document.md").expect("fixture should exist");
    let theme = load_theme();
    let content = markdown_to_typst(&markdown);
    let world = MluxWorld::new(&theme, &content, 800.0);
    let png_data = render_to_png(&world, 144.0).expect("rendering should succeed");

    // Check PNG magic bytes
    assert_eq!(&png_data[..8], b"\x89PNG\r\n\x1a\n", "output should be valid PNG");

    // Full document should produce a substantial image
    assert!(
        png_data.len() > 5000,
        "PNG should be larger than 5KB for full document, got {} bytes",
        png_data.len()
    );
}

#[test]
fn test_convert_escapes_typst_chars() {
    let md = "Price is $100 and use #hashtag";
    let typst = markdown_to_typst(md);
    assert!(typst.contains("\\$100"), "$ should be escaped");
    assert!(typst.contains("\\#hashtag"), "# should be escaped");
}
