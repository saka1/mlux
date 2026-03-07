use std::fmt::Write as _;
use std::time::Instant;

use anyhow::{Result, bail};
use log::info;

use super::convert::SourceMap;
use super::world::{FontCache, MluxWorld};
use crate::tile::{SourceMappingParams, TiledDocument, VisualLine, extract_visual_lines_with_map};

/// Default sidebar width in typst points (used by cmd_render).
pub const DEFAULT_SIDEBAR_WIDTH_PT: f64 = 40.0;

/// Parameters for [`build_tiled_document`].
pub struct BuildParams<'a> {
    pub theme_text: &'a str,
    pub data_files: crate::theme::DataFiles,
    pub content_text: &'a str,
    pub md_source: &'a str,
    pub source_map: &'a SourceMap,
    pub width_pt: f64,
    pub sidebar_width_pt: f64,
    pub tile_height_pt: f64,
    pub ppi: f32,
    pub fonts: &'a FontCache,
    pub image_files: crate::image::LoadedImages,
}

/// Build a TiledDocument from converted Typst content.
///
/// Shared pipeline used by both `cmd_render` and the terminal viewer.
/// Compiles content, extracts visual lines with source mapping,
/// generates + compiles sidebar, and assembles into a TiledDocument.
pub fn build_tiled_document(params: &BuildParams<'_>) -> Result<TiledDocument> {
    let BuildParams {
        theme_text,
        data_files,
        content_text,
        md_source,
        source_map,
        width_pt,
        sidebar_width_pt,
        tile_height_pt,
        ppi,
        fonts,
        image_files,
    } = params;
    let start = Instant::now();

    // 1. Compile content document
    let content_world = MluxWorld::new(
        theme_text,
        data_files,
        content_text,
        *width_pt,
        fonts,
        image_files.clone(),
    );
    let document = super::render::compile_document(&content_world)?;

    // 2. Extract visual lines with source mapping
    let mapping_params = SourceMappingParams {
        source: content_world.main_source(),
        content_offset: content_world.content_offset(),
        source_map,
        md_source,
    };
    let visual_lines = extract_visual_lines_with_map(&document, *ppi, Some(&mapping_params));

    if document.pages.is_empty() {
        bail!("[BUG] document has no pages");
    }
    let page_height_pt = document.pages[0].frame.size().y.to_pt();

    // 3. Compile sidebar document using visual lines
    let sidebar_source = generate_sidebar_typst(&visual_lines, *sidebar_width_pt, page_height_pt);
    let sidebar_world = MluxWorld::new_raw(&sidebar_source, fonts);
    let sidebar_doc = super::render::compile_document(&sidebar_world)?;

    // 4. Build TiledDocument with both content + sidebar
    let tiled_doc =
        TiledDocument::new(&document, &sidebar_doc, visual_lines, *tile_height_pt, *ppi)?;
    info!(
        "build_tiled_document completed in {:.1}ms",
        start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(tiled_doc)
}

/// Generate Typst source for the sidebar image.
///
/// Uses `#place()` to position line numbers at the exact Y coordinates
/// extracted from the content document's frame tree.
pub fn generate_sidebar_typst(
    lines: &[VisualLine],
    sidebar_width_pt: f64,
    page_height_pt: f64,
) -> String {
    let mut src = String::new();
    writeln!(
        src,
        "#set page(width: {sidebar_width_pt:.1}pt, height: {page_height_pt:.1}pt, margin: 0pt, fill: rgb(\"#1e1e2e\"))"
    )
    .unwrap();
    writeln!(
        src,
        "#set text(font: \"DejaVu Sans Mono\", size: 8pt, fill: rgb(\"#6c7086\"))"
    )
    .unwrap();

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        let dy = line.y_pt;
        // Place at baseline Y; use top+right alignment with ascent offset.
        // 8pt text has ~6pt ascent, so shift up to align baselines.
        writeln!(
            src,
            "#place(top + right, dy: {dy:.1}pt - 6pt, dx: -4pt)[#text(size: 8pt)[{line_num}]]"
        )
        .unwrap();
    }

    src
}
