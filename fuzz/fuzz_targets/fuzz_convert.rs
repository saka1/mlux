#![no_main]

use libfuzzer_sys::fuzz_target;
use mlux::convert::{markdown_to_typst, markdown_to_typst_with_map};

fuzz_target!(|data: &[u8]| {
    let Ok(markdown) = std::str::from_utf8(data) else {
        return;
    };

    // Run both APIs — they must not panic.
    let simple = markdown_to_typst(markdown);
    let (with_map, source_map) = markdown_to_typst_with_map(markdown);

    // Both must produce identical Typst output.
    assert_eq!(simple, with_map);

    // SourceMap ranges must be within bounds.
    for block in &source_map.blocks {
        assert!(
            block.typst_byte_range.end <= with_map.len(),
            "typst_byte_range {:?} out of bounds (len={})",
            block.typst_byte_range,
            with_map.len(),
        );
        assert!(
            block.md_byte_range.end <= markdown.len(),
            "md_byte_range {:?} out of bounds (len={})",
            block.md_byte_range,
            markdown.len(),
        );
        assert!(
            block.typst_byte_range.start <= block.typst_byte_range.end,
            "typst_byte_range inverted: {:?}",
            block.typst_byte_range,
        );
        assert!(
            block.md_byte_range.start <= block.md_byte_range.end,
            "md_byte_range inverted: {:?}",
            block.md_byte_range,
        );
    }

    // Blocks must be sorted by typst_byte_range.start and non-overlapping.
    for pair in source_map.blocks.windows(2) {
        assert!(
            pair[0].typst_byte_range.end <= pair[1].typst_byte_range.start,
            "overlapping typst ranges: {:?} and {:?}",
            pair[0].typst_byte_range,
            pair[1].typst_byte_range,
        );
    }

    // typst/comemo のメモ化キャッシュをクリア（RSS 蓄積防止）
    comemo::evict(0);
});
