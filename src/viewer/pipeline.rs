//! Document build pipeline: Markdown → Typst → TiledDocument.

use super::state::Layout;
use crate::tile::TiledDocument;
use crate::world::FontCache;

pub(super) fn build_tiled_document(
    theme_text: &str,
    content_text: &str,
    md_source: &str,
    source_map: &crate::convert::SourceMap,
    layout: &Layout,
    ppi: f32,
    tile_height_min: f64,
    fonts: &FontCache,
) -> anyhow::Result<TiledDocument> {
    let viewport_px_w = layout.image_cols as f64 * layout.cell_w as f64;
    let width_pt = viewport_px_w * 72.0 / ppi as f64;

    // Tile must be at least as tall as viewport to avoid scaling artifacts.
    let vp_height_pt = layout.image_rows as f64 * layout.cell_h as f64 * 72.0 / ppi as f64;
    let tile_height_pt = tile_height_min.max(vp_height_pt);

    let sidebar_width_pt = layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / ppi as f64;

    crate::tile::build_tiled_document(
        theme_text,
        content_text,
        md_source,
        source_map,
        width_pt,
        sidebar_width_pt,
        tile_height_pt,
        ppi,
        fonts,
    )
}
