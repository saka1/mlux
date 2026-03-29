mod highlight;
mod render_png;
mod tile;
mod tile_cache;
mod visual_line;

pub use highlight::{
    HIGHLIGHT_ACTIVE_PNG, HIGHLIGHT_PNG, HIGHLIGHT_PNG_HEIGHT, HIGHLIGHT_PNG_WIDTH, HighlightRect,
    HighlightSpec, PATTERN_ACTIVE_P25, PATTERN_ACTIVE_P50, PATTERN_ACTIVE_P75, PATTERN_HEIGHT,
    PATTERN_P25, PATTERN_P50, PATTERN_P75, PATTERN_WIDTH, PartialPattern, find_highlight_rects,
    select_overflow_pattern,
};
pub use render_png::render_frame_to_png;
pub use tile::{
    ContentMapping, DocumentMeta, TileHash, TilePngs, TiledDocument, VisibleTiles,
    compute_tile_pair_hash, split_frame,
};
pub use tile_cache::TileCache;
pub use visual_line::{
    VisualLine, byte_offset_to_line, extract_visual_lines, extract_visual_lines_with_map,
};
