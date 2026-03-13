use std::fs;

use mlux::image::LoadedImages;
use mlux::pipeline::{
    FontCache, MluxWorld, compile_document, markdown_to_typst, render_frame_to_png,
};
use mlux::tile::{
    SourceMappingParams, extract_visual_lines_with_map, split_frame, yank_exact, yank_lines,
};

fn load_theme() -> &'static str {
    mlux::theme::get("catppuccin").expect("built-in theme should exist")
}

#[test]
fn test_paragraph_ja_renders() {
    let markdown =
        fs::read_to_string("tests/fixtures/01_paragraph_ja.md").expect("fixture should exist");
    let theme = load_theme();
    let content = markdown_to_typst(&markdown, None).0;
    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        800.0,
        &font_cache,
        LoadedImages::default(),
    );
    let document = compile_document(&world).expect("compilation should succeed");
    let tiles = split_frame(&document.pages[0].frame, 500.0);
    let png_data = render_frame_to_png(&tiles[0], &document.pages[0].fill, 144.0)
        .expect("rendering should succeed");

    // Check PNG magic bytes
    assert_eq!(
        &png_data[..8],
        b"\x89PNG\r\n\x1a\n",
        "output should be valid PNG"
    );

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
    let content = markdown_to_typst("", None).0;
    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        800.0,
        &font_cache,
        LoadedImages::default(),
    );
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
    let content = markdown_to_typst(&markdown, None).0;
    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        800.0,
        &font_cache,
        LoadedImages::default(),
    );
    let document = compile_document(&world).expect("compilation should succeed");
    let tiles = split_frame(&document.pages[0].frame, 500.0);

    // Should produce multiple tiles for a full document
    assert!(!tiles.is_empty(), "should produce at least one tile");

    // Check first tile renders to valid PNG
    let png_data = render_frame_to_png(&tiles[0], &document.pages[0].fill, 144.0)
        .expect("rendering should succeed");
    assert_eq!(
        &png_data[..8],
        b"\x89PNG\r\n\x1a\n",
        "output should be valid PNG"
    );

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
    let typst = markdown_to_typst(md, None).0;
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
    let (content, source_map) = markdown_to_typst(md, None);
    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        WIDTH_PT,
        &font_cache,
        LoadedImages::default(),
    );
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
            vl.md_line_range.is_some_and(|(s, _)| s >= 3) // code block starts at line 3
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
    let md = "Text with **bold** and [link](http://example.invalid/).\n";
    let vlines = source_map_pipeline(md);
    assert!(!vlines.is_empty(), "expected at least 1 visual line");

    let yanked = yank_lines(md, &vlines, 0, 0);
    assert_eq!(
        yanked,
        "Text with **bold** and [link](http://example.invalid/)."
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
        .filter(|(_, vl)| vl.md_line_range.is_some_and(|(s, _)| s >= 3))
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
        assert_eq!(
            exact, expected,
            "yank_exact vl {idx} should match md line {exact_line}"
        );
    }
}

#[test]
fn test_yank_exact_single_line_paragraph() {
    // Paragraphs that follow other blocks have a Typst separator (\n) prepended,
    // causing a newline mismatch → md_line_exact = None → falls back to yank_lines.
    // This is correct: the safety check prevents false exactness.
    let md = "# H\n\nA simple paragraph.\n";
    let vlines = source_map_pipeline(md);

    // Find the paragraph visual line (not heading)
    let para_idx = vlines
        .iter()
        .position(|vl| vl.md_line_range.is_some_and(|(s, _)| s >= 3))
        .expect("should find paragraph visual line");

    // yank_exact should fall back to block yank (same result)
    let exact = yank_exact(md, &vlines, para_idx);
    let block = yank_lines(md, &vlines, para_idx, para_idx);
    assert_eq!(
        exact, block,
        "yank_exact should fall back to yank_lines for paragraph after other blocks"
    );
}

#[test]
fn test_yank_exact_vs_block_for_code() {
    let md = "# H\n\n```\nline1\nline2\nline3\n```\n";
    let vlines = source_map_pipeline(md);

    // Find a code block visual line (md_line_range starts at line 3+ for the code block)
    let code_vl = vlines
        .iter()
        .position(|vl| vl.md_line_exact.is_some() && vl.md_line_range.is_some_and(|(s, _)| s >= 3))
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

#[test]
fn test_yank_exact_list_items() {
    let md = "- Item 1\n- Item 2\n- Item 3\n";
    let vlines = source_map_pipeline(md);

    // Each list item should have md_line_exact set
    let list_vls: Vec<usize> = vlines
        .iter()
        .enumerate()
        .filter(|(_, vl)| vl.md_line_range.is_some())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        list_vls.len(),
        3,
        "expected 3 visual lines for 3 list items"
    );

    for &idx in &list_vls {
        assert!(
            vlines[idx].md_line_exact.is_some(),
            "list item vl {idx} should have md_line_exact"
        );
    }

    // yank_exact should return individual items
    assert_eq!(yank_exact(md, &vlines, list_vls[0]), "- Item 1");
    assert_eq!(yank_exact(md, &vlines, list_vls[1]), "- Item 2");
    assert_eq!(yank_exact(md, &vlines, list_vls[2]), "- Item 3");

    // yank_lines should still return the whole list (unchanged behavior)
    let block = yank_lines(md, &vlines, list_vls[0], list_vls[0]);
    assert_eq!(block, "- Item 1\n- Item 2\n- Item 3");
}

#[test]
fn test_yank_exact_ordered_list_items() {
    let md = "1. First\n2. Second\n3. Third\n";
    let vlines = source_map_pipeline(md);

    let list_vls: Vec<usize> = vlines
        .iter()
        .enumerate()
        .filter(|(_, vl)| vl.md_line_range.is_some())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        list_vls.len(),
        3,
        "expected 3 visual lines for 3 ordered list items"
    );

    assert_eq!(yank_exact(md, &vlines, list_vls[0]), "1. First");
    assert_eq!(yank_exact(md, &vlines, list_vls[1]), "2. Second");
    assert_eq!(yank_exact(md, &vlines, list_vls[2]), "3. Third");
}

#[test]
fn test_yank_exact_table_fallback() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
    let vlines = source_map_pipeline(md);

    // Tables have mismatched newline counts — should fall back to None
    for (i, vl) in vlines.iter().enumerate() {
        if vl.md_line_range.is_some() {
            assert!(
                vl.md_line_exact.is_none(),
                "table vl {i} should NOT have md_line_exact (newline mismatch)"
            );
        }
    }
}

#[test]
fn test_yank_exact_blockquote_fallback() {
    let md = "> Quote line 1\n> Quote line 2\n";
    let vlines = source_map_pipeline(md);

    // Blockquotes have mismatched newline counts — should fall back to None
    for (i, vl) in vlines.iter().enumerate() {
        if vl.md_line_range.is_some() {
            assert!(
                vl.md_line_exact.is_none(),
                "blockquote vl {i} should NOT have md_line_exact (newline mismatch)"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Structural visual line extraction tests
// ---------------------------------------------------------------------------

#[test]
fn test_display_math_single_visual_line() {
    // A single display math block should produce exactly 1 visual line.
    // This was the main bug in the old flat-TextItem approach (Y tolerance splitting).
    let md = "$$\\det(A) = \\sum_{\\sigma \\in S_n} \\varepsilon(\\sigma) \\prod_{i=1}^{n} a_{i,\\sigma(i)}$$\n";
    let vlines = source_map_pipeline(md);
    assert_eq!(
        vlines.len(),
        1,
        "single display math should produce 1 visual line, got {}",
        vlines.len()
    );
    assert!(
        vlines[0].md_line_range.is_some(),
        "display math visual line should have md_line_range"
    );

    // Formula with sub/superscripts should also be 1 visual line
    let md2 = "$$e^{i\\theta} = \\cos\\theta + i\\sin\\theta$$\n";
    let vlines2 = source_map_pipeline(md2);
    assert_eq!(
        vlines2.len(),
        1,
        "display math with sub/superscripts (Euler) should produce 1 visual line, got {}",
        vlines2.len()
    );
}

#[test]
fn test_inline_math_no_line_split() {
    // Inline math should not cause extra visual lines
    let md_no_math = "The eigenvalue problem is called the characteristic equation.\n";
    let md_with_math = "The eigenvalue problem $\\det(A - \\lambda I) = 0$ is called the characteristic equation.\n";

    let vlines_no = source_map_pipeline(md_no_math);
    let vlines_with = source_map_pipeline(md_with_math);
    assert_eq!(
        vlines_no.len(),
        vlines_with.len(),
        "inline math should not change visual line count: without={}, with={}",
        vlines_no.len(),
        vlines_with.len()
    );
}

#[test]
fn test_math_showcase_visual_line_count() {
    let md =
        fs::read_to_string("tests/fixtures/10_math_showcase.md").expect("fixture should exist");
    let vlines = source_map_pipeline(&md);

    // The old flat approach produced ~151 visual lines due to Y tolerance splitting
    // math into many fragments. The structural approach keeps math blocks whole
    // (though long formulas still wrap). Should be well under the old count.
    assert!(
        vlines.len() < 151,
        "math showcase should produce fewer visual lines than old approach (151), got {}",
        vlines.len()
    );
    assert!(
        vlines.len() >= 30,
        "math showcase should produce at least 30 visual lines, got {}",
        vlines.len()
    );

    // Every visual line should have source mapping
    for (i, vl) in vlines.iter().enumerate() {
        assert!(
            vl.md_line_range.is_some(),
            "math showcase vl {i} should have md_line_range, y_pt={:.1}",
            vl.y_pt
        );
    }
}

#[test]
fn test_code_block_lines_are_individual() {
    let md = "# H\n\n```rust\nlet a = 1;\nlet b = 2;\nlet c = 3;\n```\n";
    let vlines = source_map_pipeline(md);

    // Filter to code block visual lines (md_line_range starting at line 3+)
    let code_vls: Vec<usize> = vlines
        .iter()
        .enumerate()
        .filter(|(_, vl)| vl.md_line_range.is_some_and(|(s, _)| s >= 3))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        code_vls.len(),
        3,
        "3-line code block should produce 3 visual lines, got {}",
        code_vls.len()
    );

    // Each should have md_line_exact for per-line yank
    for &idx in &code_vls {
        assert!(
            vlines[idx].md_line_exact.is_some(),
            "code block visual line {idx} should have md_line_exact"
        );
    }
}

#[test]
fn test_list_items_are_individual_visual_lines() {
    let md = "- Apple\n- Banana\n- Cherry\n- Date\n";
    let vlines = source_map_pipeline(md);

    let list_vls: Vec<&mlux::tile::VisualLine> = vlines
        .iter()
        .filter(|vl| vl.md_line_range.is_some())
        .collect();
    assert_eq!(
        list_vls.len(),
        4,
        "4 list items should produce 4 visual lines, got {}",
        list_vls.len()
    );
}

#[test]
fn test_table_rows_are_individual_visual_lines() {
    let md = "| H1 | H2 |\n|---|---|\n| A | B |\n| C | D |\n| E | F |\n";
    let vlines = source_map_pipeline(md);

    let table_vls: Vec<&mlux::tile::VisualLine> = vlines
        .iter()
        .filter(|vl| vl.md_line_range.is_some())
        .collect();
    // Header + 3 data rows = at least 4 visual lines
    assert!(
        table_vls.len() >= 4,
        "table with header + 3 rows should produce >=4 visual lines, got {}",
        table_vls.len()
    );
}

#[test]
fn test_nested_blockquote_known_limitation() {
    // Nested blockquotes currently collapse into a single visual line.
    // This documents the known limitation.
    let md = "> Outer\n> > Inner\n> > More inner\n";
    let vlines = source_map_pipeline(md);

    let quote_vls: Vec<&mlux::tile::VisualLine> = vlines
        .iter()
        .filter(|vl| vl.md_line_range.is_some())
        .collect();
    assert_eq!(
        quote_vls.len(),
        1,
        "nested blockquote currently produces 1 visual line (known limitation), got {}",
        quote_vls.len()
    );
}

#[test]
fn test_heading_single_visual_line() {
    let md = "## Section Title\n";
    let vlines = source_map_pipeline(md);
    assert_eq!(
        vlines.len(),
        1,
        "single heading should produce 1 visual line, got {}",
        vlines.len()
    );
    assert!(vlines[0].md_line_range.is_some());
}

#[test]
fn test_horizontal_rule_no_visual_line() {
    // Horizontal rules are Shapes (not text), so they don't produce visual lines.
    // Only the surrounding text should appear.
    let md = "Above\n\n---\n\nBelow\n";
    let vlines = source_map_pipeline(md);

    // Should have exactly 2 visual lines: "Above" and "Below"
    assert_eq!(
        vlines.len(),
        2,
        "hr doc should produce 2 visual lines (above + below), got {}",
        vlines.len()
    );

    let yanked_first = yank_lines(md, &vlines, 0, 0);
    assert_eq!(yanked_first, "Above");
    let yanked_last = yank_lines(md, &vlines, 1, 1);
    assert_eq!(yanked_last, "Below");
}

#[test]
fn test_all_features_renders() {
    let markdown =
        fs::read_to_string("tests/fixtures/09_all_features.md").expect("fixture should exist");
    let theme = load_theme();
    let content = markdown_to_typst(&markdown, None).0;
    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        800.0,
        &font_cache,
        LoadedImages::default(),
    );
    let document = compile_document(&world).expect("compilation should succeed");
    let tiles = split_frame(&document.pages[0].frame, 500.0);

    // Comprehensive document should produce multiple tiles
    assert!(
        tiles.len() >= 3,
        "all-features document should produce at least 3 tiles, got {}",
        tiles.len()
    );

    // Verify every tile renders to valid PNG
    for (i, tile) in tiles.iter().enumerate() {
        let png_data = render_frame_to_png(tile, &document.pages[0].fill, 144.0)
            .unwrap_or_else(|e| panic!("tile {i} should render: {e}"));
        assert_eq!(
            &png_data[..8],
            b"\x89PNG\r\n\x1a\n",
            "tile {i} should be valid PNG"
        );
        assert!(
            png_data.len() > 1000,
            "tile {i} PNG should be >1KB, got {} bytes",
            png_data.len()
        );
    }
}

#[test]
fn test_all_features_source_map() {
    let md = fs::read_to_string("tests/fixtures/09_all_features.md").expect("fixture should exist");
    let vlines = source_map_pipeline(&md);

    // Should have many visual lines
    assert!(
        vlines.len() > 20,
        "expected >20 visual lines for all-features document, got {}",
        vlines.len()
    );

    // First visual line should be the main heading
    let yanked_first = yank_lines(&md, &vlines, 0, 0);
    assert_eq!(yanked_first, "# mlux 全機能テストドキュメント");
}

#[test]
fn test_image_renders() {
    let markdown = fs::read_to_string("tests/fixtures/11_image.md").expect("fixture should exist");
    let theme = load_theme();

    // Load images (same flow as cmd_render)
    let base_dir = std::path::Path::new("tests/fixtures");
    let image_paths = mlux::pipeline::extract_image_paths(&markdown);
    let (image_files, errors) = mlux::image::load_images(&image_paths, Some(base_dir), false);
    assert!(errors.is_empty(), "image load errors: {errors:?}");
    assert!(
        image_files.get("test_image.png").is_some(),
        "should have loaded test_image.png"
    );

    let loaded_set = image_files.key_set();
    let (content, _source_map) = markdown_to_typst(&markdown, Some(&loaded_set));

    // Verify #image() is in the generated Typst
    assert!(
        content.contains("#image(\"test_image.png\""),
        "should contain #image() call, got: {content}"
    );

    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        800.0,
        &font_cache,
        image_files,
    );
    let document = compile_document(&world).expect("compilation should succeed with image");
    let tiles = split_frame(&document.pages[0].frame, 500.0);
    assert!(!tiles.is_empty(), "should produce at least one tile");

    let png_data = render_frame_to_png(&tiles[0], &document.pages[0].fill, 144.0)
        .expect("rendering should succeed");
    assert_eq!(
        &png_data[..8],
        b"\x89PNG\r\n\x1a\n",
        "output should be valid PNG"
    );
    assert!(
        png_data.len() > 1000,
        "PNG should be >1KB with image, got {} bytes",
        png_data.len()
    );
}

#[test]
fn test_mermaid_diagram_renders() {
    let md =
        "# Mermaid Test\n\n```mermaid\ngraph LR\n  A --> B\n  B --> C\n```\n\nSome text after.\n";
    let theme = load_theme();

    // Extract and render diagrams
    let diagrams = mlux::diagram::extract_diagrams(md);
    assert_eq!(diagrams.len(), 1, "should extract 1 mermaid diagram");

    let rendered =
        mlux::diagram::render_diagrams(&diagrams, mlux::theme::mermaid_colors("catppuccin"));
    assert_eq!(rendered.len(), 1, "should render 1 diagram to SVG");

    // Build image set with rendered diagrams
    let mut image_files = LoadedImages::default();
    for (key, svg) in rendered {
        image_files.insert(key, svg);
    }
    let loaded_set = image_files.key_set();

    // Convert markdown to typst — mermaid block should become #image()
    let (content, _source_map) = markdown_to_typst(md, Some(&loaded_set));
    assert!(
        content.contains("#image(\"_diagram_"),
        "mermaid block should produce #image() call, got: {content}"
    );
    assert!(
        !content.contains("```mermaid"),
        "mermaid block should not remain as code fence"
    );

    // Full render
    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        800.0,
        &font_cache,
        image_files,
    );
    let document = compile_document(&world).expect("compilation should succeed with mermaid");
    let tiles = split_frame(&document.pages[0].frame, 500.0);
    assert!(!tiles.is_empty(), "should produce at least one tile");

    let png_data = render_frame_to_png(&tiles[0], &document.pages[0].fill, 144.0)
        .expect("rendering should succeed");
    assert_eq!(
        &png_data[..8],
        b"\x89PNG\r\n\x1a\n",
        "output should be valid PNG"
    );
    assert!(
        png_data.len() > 1000,
        "PNG should be >1KB with mermaid diagram, got {} bytes",
        png_data.len()
    );
}

#[test]
fn test_mermaid_fixture_renders() {
    let markdown =
        fs::read_to_string("tests/fixtures/13_mermaid.md").expect("fixture should exist");
    let theme = load_theme();

    // Full pipeline: extract diagrams, render, convert, compile
    let diagrams = mlux::diagram::extract_diagrams(&markdown);
    assert!(
        diagrams.len() >= 3,
        "fixture should have at least 3 mermaid blocks, got {}",
        diagrams.len()
    );

    let rendered =
        mlux::diagram::render_diagrams(&diagrams, mlux::theme::mermaid_colors("catppuccin"));
    assert_eq!(
        rendered.len(),
        diagrams.len(),
        "all diagrams should render successfully"
    );

    let mut image_files = LoadedImages::default();
    for (key, svg) in rendered {
        image_files.insert(key, svg);
    }
    let loaded_set = image_files.key_set();

    let (content, _source_map) = markdown_to_typst(&markdown, Some(&loaded_set));

    // All mermaid blocks should become #image() calls
    assert!(
        !content.contains("```mermaid"),
        "no mermaid code fences should remain"
    );
    // Normal code blocks should be preserved
    assert!(
        content.contains("```rust"),
        "rust code block should be preserved"
    );

    let font_cache = FontCache::new();
    let world = MluxWorld::new(
        theme,
        mlux::theme::data_files("catppuccin"),
        &content,
        800.0,
        &font_cache,
        image_files,
    );
    let document = compile_document(&world).expect("compilation should succeed");
    let tiles = split_frame(&document.pages[0].frame, 500.0);
    assert!(
        tiles.len() >= 2,
        "mermaid fixture should produce multiple tiles, got {}",
        tiles.len()
    );

    // Verify all tiles render to valid PNG
    for (i, tile) in tiles.iter().enumerate() {
        let png_data = render_frame_to_png(tile, &document.pages[0].fill, 144.0)
            .unwrap_or_else(|e| panic!("tile {i} should render: {e}"));
        assert_eq!(
            &png_data[..8],
            b"\x89PNG\r\n\x1a\n",
            "tile {i} should be valid PNG"
        );
    }
}

#[test]
fn test_mermaid_fallback_without_render() {
    let md = "```mermaid\ngraph LR\n  A --> B\n```\n";

    // Without rendering diagrams, mermaid blocks should fall back to code fence
    let (content, _source_map) = markdown_to_typst(md, None);
    assert!(
        content.contains("```mermaid"),
        "without available images, mermaid should render as code fence, got: {content}"
    );
}

#[test]
fn test_inline_code_no_line_overlap() {
    // Regression test: inline code with `inset: (y: 2pt)` increased layout box height,
    // causing background rectangles to overlap the next text line. The fix uses
    // `outset: (y: 2pt)` instead, which only expands the painted area without
    // affecting layout. We verify that inline code doesn't compress line spacing
    // compared to plain text.
    let md_plain = "This is a plain paragraph that should wrap across multiple visual lines when rendered at a narrow width for testing purposes here.\n";
    let md_code = "This has `inline_code` in a paragraph that should wrap across multiple visual lines when rendered at a narrow width for testing purposes here.\n";

    let vlines_plain = source_map_pipeline(md_plain);
    let vlines_code = source_map_pipeline(md_code);

    // Both should wrap into multiple visual lines at WIDTH_PT=400
    assert!(
        vlines_plain.len() >= 2,
        "plain paragraph should wrap into >=2 visual lines, got {}",
        vlines_plain.len()
    );
    assert!(
        vlines_code.len() >= 2,
        "paragraph with inline code should wrap into >=2 visual lines, got {}",
        vlines_code.len()
    );

    // Line spacing with inline code should be the same as without.
    // With the old `inset: (y: 2pt)`, inline code boxes were taller and caused
    // uneven/compressed spacing that led to visual overlap.
    let spacing_plain = vlines_plain[1].y_pt - vlines_plain[0].y_pt;
    let spacing_code = vlines_code[1].y_pt - vlines_code[0].y_pt;
    assert!(
        (spacing_code - spacing_plain).abs() < 1.0,
        "inline code should not change line spacing: plain={spacing_plain:.1}pt, code={spacing_code:.1}pt"
    );
}

// ---------------------------------------------------------------------------
// Content-addressed tile hash / merge tests
// ---------------------------------------------------------------------------

use mlux::pipeline::build_tiled_document;
use mlux::tile::{TilePairHash, TiledDocumentCache, merge_tile_cache};

/// Build a TiledDocument from markdown, returning metadata with hashes.
fn build_hashes(md: &str) -> Vec<TilePairHash> {
    let theme = load_theme();
    let font_cache = FontCache::new();
    let params = mlux::pipeline::BuildParams {
        theme_name: "catppuccin",
        theme_text: theme,
        data_files: mlux::theme::data_files("catppuccin"),
        markdown: md,
        base_dir: None,
        width_pt: WIDTH_PT,
        sidebar_width_pt: 50.0,
        tile_height_pt: 200.0,
        ppi: PPI,
        fonts: &font_cache,
        allow_remote_images: false,
    };
    let doc = build_tiled_document(&params).expect("build should succeed");
    let meta = doc.metadata();
    assert!(
        !meta.tile_hashes.is_empty(),
        "metadata should contain tile hashes"
    );
    meta.tile_hashes
}

#[test]
fn test_tile_hash_identical_builds_match() {
    let md = "# Hello\n\nWorld\n\nParagraph two.\n";
    let h1 = build_hashes(md);
    let h2 = build_hashes(md);
    assert_eq!(h1.len(), h2.len());
    for i in 0..h1.len() {
        assert_eq!(
            h1[i], h2[i],
            "tile {i} hash should be identical for identical input"
        );
    }
}

#[test]
fn test_tile_hash_merge_recovers_unchanged_tiles() {
    // Build original document with multiple tiles
    let lines: Vec<String> = (0..50).map(|i| format!("Line {i}\n")).collect();
    let md_original: String = lines.iter().cloned().collect();

    let old_hashes = build_hashes(&md_original);
    let mut old_cache = TiledDocumentCache::new();
    // Simulate rendering all tiles
    for i in 0..old_hashes.len() {
        old_cache.insert(
            i,
            mlux::tile::TilePngs {
                content: vec![i as u8],
                sidebar: vec![i as u8],
            },
        );
    }

    // Modify only the last line
    let mut md_modified = md_original.clone();
    md_modified.push_str("Extra line at the end.\n");

    let new_hashes = build_hashes(&md_modified);
    let total = new_hashes.len();
    let new_cache = merge_tile_cache(&new_hashes, &old_hashes, &mut old_cache);

    // At least some tiles should be recovered (early tiles are unchanged)
    assert!(
        !new_cache.is_empty(),
        "merge should recover at least some tiles (recovered {}/{total})",
        new_cache.len()
    );
}

#[test]
fn test_tile_hash_no_change_full_recovery() {
    let md = "# Title\n\nSome content.\n";
    let hashes = build_hashes(md);
    let mut old_cache = TiledDocumentCache::new();
    for i in 0..hashes.len() {
        old_cache.insert(
            i,
            mlux::tile::TilePngs {
                content: vec![i as u8],
                sidebar: vec![i as u8],
            },
        );
    }

    // Rebuild identical document
    let total = hashes.len();
    let new_cache = merge_tile_cache(&hashes, &hashes, &mut old_cache);
    assert_eq!(
        new_cache.len(),
        total,
        "identical rebuild should recover all {total} tiles"
    );
}

#[test]
fn test_code_block_no_lang_lines_are_individual() {
    let md = "# H\n\n```\nline1\nline2\nline3\n```\n";
    let vlines = source_map_pipeline(md);

    let code_vls: Vec<usize> = vlines
        .iter()
        .enumerate()
        .filter(|(_, vl)| {
            vl.md_line_exact.is_some() && vl.md_line_range.is_some_and(|(s, _)| s >= 3)
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        code_vls.len(),
        3,
        "3-line no-language code block should produce 3 visual lines with md_line_exact, got {}",
        code_vls.len()
    );
}

#[test]
fn test_code_block_unrecognized_lang_lines_are_individual() {
    let md = "# H\n\n```console\n$ cargo build\n$ cargo test\n```\n";
    let vlines = source_map_pipeline(md);

    let code_vls: Vec<usize> = vlines
        .iter()
        .enumerate()
        .filter(|(_, vl)| {
            vl.md_line_exact.is_some() && vl.md_line_range.is_some_and(|(s, _)| s >= 3)
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        code_vls.len(),
        2,
        "2-line console code block should produce 2 visual lines with md_line_exact, got {}",
        code_vls.len()
    );
}
