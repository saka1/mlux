//! Document build pipeline: Markdown → Typst → TiledDocument.

use super::state::Layout;
use crate::tile::{BuildParams, TiledDocument};
use crate::world::FontCache;

pub(super) struct PipelineInput<'a> {
    pub theme_text: &'a str,
    pub data_files: crate::theme::DataFiles,
    pub content_text: &'a str,
    pub md_source: &'a str,
    pub source_map: &'a crate::convert::SourceMap,
    pub layout: &'a Layout,
    pub ppi: f32,
    pub tile_height_min: f64,
    pub fonts: &'a FontCache,
    pub image_files: crate::image::LoadedImages,
}

pub(super) fn to_build_params<'a>(input: &'a PipelineInput<'a>) -> BuildParams<'a> {
    let layout = input.layout;
    let ppi = input.ppi;

    let viewport_px_w = layout.image_cols as f64 * layout.cell_w as f64;
    let width_pt = viewport_px_w * 72.0 / ppi as f64;

    // Tile must be at least as tall as viewport to avoid scaling artifacts.
    let vp_height_pt = layout.image_rows as f64 * layout.cell_h as f64 * 72.0 / ppi as f64;
    let tile_height_pt = input.tile_height_min.max(vp_height_pt);

    let sidebar_width_pt = layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / ppi as f64;

    BuildParams {
        theme_text: input.theme_text,
        data_files: input.data_files,
        content_text: input.content_text,
        md_source: input.md_source,
        source_map: input.source_map,
        width_pt,
        sidebar_width_pt,
        tile_height_pt,
        ppi,
        fonts: input.fonts,
        image_files: input.image_files.clone(),
    }
}

pub(super) fn build_tiled_document(input: &PipelineInput<'_>) -> anyhow::Result<TiledDocument> {
    crate::tile::build_tiled_document(&to_build_params(input))
}
