use std::fs;

use mlux::convert::{markdown_to_typst, markdown_to_typst_with_map};
use mlux::render::{compile_document, render_frame_to_png};
use mlux::tile::{SourceMappingParams, extract_visual_lines_with_map, split_frame, yank_exact, yank_lines};
use mlux::world::{FontCache, MluxWorld};

fn load_theme() -> String {
    fs::read_to_string("themes/catppuccin.typ").expect("theme file should exist")
}

#[test]
fn test_paragraph_ja_renders() {
    let markdown =
        fs::read_to_string("tests/fixtures/01_paragraph_ja.md").expect("fixture should exist");
    let theme = load_theme();
    let content = markdown_to_typst(&markdown);
    let font_cache = FontCache::new();
    let world = MluxWorld::new(&theme, &content, 800.0, &font_cache);
    let document = compile_document(&world).expect("compilation should succeed");
    let tiles = split_frame(&document.pages[0].frame, 500.0);
    let png_data = render_frame_to_png(&tiles[0], &document.pages[0].fill, 144.0)
        .expect("rendering should succeed");

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
    let font_cache = FontCache::new();
    let world = MluxWorld::new(&theme, &content, 800.0, &font_cache);
    let document = compile_document(&world).expect("compilation should succeed");
    let tiles = split_frame(&document.pages[0].frame, 500.0);
    let png_data = render_frame_to_png(&tiles[0], &document.pages[0].fill, 144.0)
        .expect("empty input should still render");

    assert_eq!(&png_data[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn test_full_document_renders() {
    let markdown =
        fs::read_to_string("tests/fixtures/07_full_document.md").expect("fixture should exist");
    let theme = load_theme();
    let content = markdown_to_typst(&markdown);
    let font_cache = FontCache::new();
    let world = MluxWorld::new(&theme, &content, 800.0, &font_cache);
    let document = compile_document(&world).expect("compilation should succeed");
    let tiles = split_frame(&document.pages[0].frame, 500.0);

    // Should produce multiple tiles for a full document
    assert!(
        !tiles.is_empty(),
        "should produce at least one tile"
    );

    // Check first tile renders to valid PNG
    let png_data = render_frame_to_png(&tiles[0], &document.pages[0].fill, 144.0)
        .expect("rendering should succeed");
    assert_eq!(&png_data[..8], b"\x89PNG\r\n\x1a\n", "output should be valid PNG");

    // First tile of full document should produce a substantial image
    assert!(
        png_data.len() > 5000,
        "PNG should be larger than 5KB for full document tile, got {} bytes",
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

// ---------------------------------------------------------------------------
// Source mapping integration tests
// ---------------------------------------------------------------------------

const PPI: f32 = 144.0;
const WIDTH_PT: f64 = 400.0;

/// Run the full source mapping pipeline for a given Markdown string.
///
/// Returns (visual_lines, md_source) for use in yank_lines.
fn source_map_pipeline(md: &str) -> Vec<mlux::tile::VisualLine> {
    let _ = env_logger::try_init();
    let theme = load_theme();
    let (content, source_map) = markdown_to_typst_with_map(md);
    let font_cache = FontCache::new();
    let world = MluxWorld::new(&theme, &content, WIDTH_PT, &font_cache);
    let document = compile_document(&world).expect("compilation should succeed");

    let params = SourceMappingParams {
        source: world.main_source(),
        content_offset: world.content_offset(),
        source_map: &source_map,
        md_source: md,
    };
    extract_visual_lines_with_map(&document, PPI, Some(&params))
}

#[test]
fn test_source_map_heading() {
    let md = "# Heading\n\nParagraph.\n";
    let vlines = source_map_pipeline(md);
    assert!(
        vlines.len() >= 2,
        "expected at least 2 visual lines, got {}",
        vlines.len()
    );

    // Yank the first visual line (heading)
    let yanked = yank_lines(md, &vlines, 0, 0);
    assert_eq!(yanked, "# Heading");
}

#[test]
fn test_source_map_paragraph() {
    let md = "# H\n\nThis is a long paragraph that will wrap across multiple lines in the rendered output.\n";
    let vlines = source_map_pipeline(md);
    assert!(
        vlines.len() >= 2,
        "expected at least 2 visual lines, got {}",
        vlines.len()
    );

    // Find a visual line that belongs to the paragraph (not heading)
    // The heading is visual line 0; paragraph starts at visual line 1+
    let yanked = yank_lines(md, &vlines, 1, 1);
    assert_eq!(
        yanked,
        "This is a long paragraph that will wrap across multiple lines in the rendered output."
    );
}

#[test]
fn test_source_map_code_block() {
    let md = "# H\n\n```rust\nfn main() {}\n```\n";
    let vlines = source_map_pipeline(md);
    assert!(
        vlines.len() >= 2,
        "expected at least 2 visual lines, got {}",
        vlines.len()
    );

    // Find a visual line inside the code block
    // Heading is vline 0, code block content starts after
    let code_vl = vlines
        .iter()
        .position(|vl| {
            vl.md_line_range
                .map_or(false, |(s, _)| s >= 3) // code block starts at line 3
        })
        .expect("should find a visual line for the code block");
    let yanked = yank_lines(md, &vlines, code_vl, code_vl);
    assert_eq!(yanked, "```rust\nfn main() {}\n```");
}

#[test]
fn test_source_map_multi_block_yank() {
    let md = "# Heading\n\nParagraph.\n\n## Sub\n";
    let vlines = source_map_pipeline(md);
    assert!(
        vlines.len() >= 3,
        "expected at least 3 visual lines, got {}",
        vlines.len()
    );

    // Yank from first to last visual line
    let last = vlines.len() - 1;
    let yanked = yank_lines(md, &vlines, 0, last);
    assert_eq!(yanked, "# Heading\n\nParagraph.\n\n## Sub");
}

#[test]
fn test_source_map_list() {
    let md = "- Item 1\n- Item 2\n- Item 3\n";
    let vlines = source_map_pipeline(md);
    assert!(
        !vlines.is_empty(),
        "expected at least 1 visual line for list"
    );

    // Yank any visual line from the list — should get the whole list
    let yanked = yank_lines(md, &vlines, 0, 0);
    assert_eq!(yanked, "- Item 1\n- Item 2\n- Item 3");
}

#[test]
fn test_source_map_ordered_list() {
    let md = "1. First\n2. Second\n3. Third\n";
    let vlines = source_map_pipeline(md);
    assert!(
        !vlines.is_empty(),
        "expected at least 1 visual line for ordered list"
    );

    // Yank any visual line from the ordered list — should get the whole list
    let yanked = yank_lines(md, &vlines, 0, 0);
    assert_eq!(yanked, "1. First\n2. Second\n3. Third");
}

#[test]
fn test_source_map_table() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
    let vlines = source_map_pipeline(md);
    assert!(
        !vlines.is_empty(),
        "expected at least 1 visual line for table"
    );

    // Yank any visual line from the table — should get the whole table
    let yanked = yank_lines(md, &vlines, 0, 0);
    assert_eq!(yanked, "| A | B |\n|---|---|\n| 1 | 2 |");
}

#[test]
fn test_source_map_blockquote() {
    let md = "> Quote line 1\n> Quote line 2\n";
    let vlines = source_map_pipeline(md);
    assert!(
        !vlines.is_empty(),
        "expected at least 1 visual line for blockquote"
    );

    let yanked = yank_lines(md, &vlines, 0, 0);
    assert_eq!(yanked, "> Quote line 1\n> Quote line 2");
}

#[test]
fn test_source_map_full_document() {
    let md =
        fs::read_to_string("tests/fixtures/07_full_document.md").expect("fixture should exist");
    let vlines = source_map_pipeline(&md);

    // Full document should have many visual lines
    assert!(
        vlines.len() > 10,
        "expected >10 visual lines for full document, got {}",
        vlines.len()
    );

    // First visual line should be the heading
    let yanked_first = yank_lines(&md, &vlines, 0, 0);
    assert_eq!(yanked_first, "# Rustにおけるエラーハンドリング");

    // Every visual line with md_line_range should produce valid yank output
    for (i, vl) in vlines.iter().enumerate() {
        if vl.md_line_range.is_some() {
            let yanked = yank_lines(&md, &vlines, i, i);
            assert!(
                !yanked.is_empty(),
                "visual line {i} has md_line_range {:?} but yank produced empty string",
                vl.md_line_range
            );
        }
    }
}

#[test]
fn test_source_map_inline_formatting_preserved() {
    let md = "Text with **bold** and [link](http://example.com).\n";
    let vlines = source_map_pipeline(md);
    assert!(
        !vlines.is_empty(),
        "expected at least 1 visual line"
    );

    let yanked = yank_lines(md, &vlines, 0, 0);
    assert_eq!(
        yanked,
        "Text with **bold** and [link](http://example.com)."
    );
}

#[test]
fn test_visual_line_count() {
    // Simple document: heading + paragraph = at least 2 visual lines
    let md = "# Title\n\nA paragraph.\n";
    let vlines = source_map_pipeline(md);
    assert!(
        vlines.len() >= 2,
        "expected at least 2 visual lines for heading + paragraph, got {}",
        vlines.len()
    );

    // Each visual line should have an md_line_range (no theme-only lines expected)
    for (i, vl) in vlines.iter().enumerate() {
        assert!(
            vl.md_line_range.is_some(),
            "visual line {i} should have md_line_range, y_pt={:.1}",
            vl.y_pt
        );
    }
}

#[test]
fn test_yank_exact_code_block_line() {
    let md = "# H\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n";
    let vlines = source_map_pipeline(md);

    // Find visual lines inside the code block (md_line_range starting at line 3+)
    let code_vls: Vec<usize> = vlines
        .iter()
        .enumerate()
        .filter(|(_, vl)| vl.md_line_range.map_or(false, |(s, _)| s >= 3))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !code_vls.is_empty(),
        "should find visual lines for code block"
    );

    // Each code block visual line should have md_line_exact set
    for &idx in &code_vls {
        assert!(
            vlines[idx].md_line_exact.is_some(),
            "code block visual line {idx} should have md_line_exact, got None"
        );
    }

    // yank_exact should return a single line, not the whole block
    for &idx in &code_vls {
        let exact = yank_exact(md, &vlines, idx);
        assert!(
            !exact.contains('\n'),
            "yank_exact for code block vl {idx} should be a single line, got: {:?}",
            exact
        );
        // The exact line should be one of the code block content lines
        let exact_line = vlines[idx].md_line_exact.unwrap();
        let expected = md.lines().nth(exact_line - 1).unwrap();
        assert_eq!(exact, expected, "yank_exact vl {idx} should match md line {exact_line}");
    }
}

#[test]
fn test_yank_exact_falls_back_for_paragraph() {
    let md = "# H\n\nA simple paragraph.\n";
    let vlines = source_map_pipeline(md);

    // Find the paragraph visual line (not heading)
    let para_idx = vlines
        .iter()
        .position(|vl| vl.md_line_range.map_or(false, |(s, _)| s >= 3))
        .expect("should find paragraph visual line");

    // Paragraph should NOT have md_line_exact
    assert!(
        vlines[para_idx].md_line_exact.is_none(),
        "paragraph visual line should have md_line_exact = None"
    );

    // yank_exact should fall back to block yank
    let exact = yank_exact(md, &vlines, para_idx);
    let block = yank_lines(md, &vlines, para_idx, para_idx);
    assert_eq!(exact, block, "yank_exact should fall back to yank_lines for paragraphs");
}

#[test]
fn test_yank_exact_vs_block_for_code() {
    let md = "# H\n\n```\nline1\nline2\nline3\n```\n";
    let vlines = source_map_pipeline(md);

    // Find a code block visual line
    let code_vl = vlines
        .iter()
        .position(|vl| vl.md_line_exact.is_some())
        .expect("should find a code block visual line with md_line_exact");

    let exact = yank_exact(md, &vlines, code_vl);
    let block = yank_lines(md, &vlines, code_vl, code_vl);

    // exact should be a single line from the code block
    assert!(
        !exact.contains('\n'),
        "yank_exact should return a single line: {:?}",
        exact
    );

    // block should contain the entire code block (with fences)
    assert!(
        block.contains("```"),
        "yank_lines should return the whole code block including fences: {:?}",
        block
    );

    // They should differ
    assert_ne!(
        exact, block,
        "yank_exact and yank_lines should differ for code blocks"
    );
}
