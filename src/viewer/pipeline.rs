//! Document build pipeline: Markdown → Typst → StripDocument.

use log::info;
use std::time::Instant;

use typst::layout::PagedDocument;

use super::state::Layout;
use crate::render::compile_document;
use crate::strip::{
    SourceMappingParams, StripDocument, VisualLine, extract_visual_lines_with_map,
    generate_sidebar_typst,
};
use crate::world::{FontCache, MluxWorld};

pub(super) const PPI: f32 = 144.0;
pub(super) const DEFAULT_STRIP_HEIGHT_PT: f64 = 500.0;

pub(super) fn build_strip_document(
    theme_text: &str,
    content_text: &str,
    md_source: &str,
    source_map: &crate::convert::SourceMap,
    layout: &Layout,
    fonts: &FontCache,
) -> anyhow::Result<StripDocument> {
    let start = Instant::now();
    let viewport_px_w = layout.image_cols as f64 * layout.cell_w as f64;
    let width_pt = viewport_px_w * 72.0 / PPI as f64;

    // Strip must be at least as tall as viewport to avoid scaling artifacts.
    let vp_height_pt = layout.image_rows as f64 * layout.cell_h as f64 * 72.0 / PPI as f64;
    let strip_height_pt = DEFAULT_STRIP_HEIGHT_PT.max(vp_height_pt);
    info!(
        "strip_height: {}pt (vp={}pt, default={}pt)",
        strip_height_pt, vp_height_pt, DEFAULT_STRIP_HEIGHT_PT
    );

    // 1. Compile content document
    let content_world = MluxWorld::new(theme_text, content_text, width_pt, fonts);
    let document = compile_document(&content_world)?;

    // 2. Extract visual lines with source mapping
    let mapping_params = SourceMappingParams {
        source: content_world.main_source(),
        content_offset: content_world.content_offset(),
        source_map,
        md_source,
    };
    let visual_lines = extract_visual_lines_with_map(&document, PPI, Some(&mapping_params));
    let page_height_pt = document.pages[0].frame.size().y.to_pt();

    let mapped = visual_lines.iter().filter(|vl| vl.md_line_range.is_some()).count();
    let unmapped = visual_lines.len() - mapped;
    info!("extract_visual_lines: {} lines ({} mapped, {} unmapped)", visual_lines.len(), mapped, unmapped);

    // 3. Compile sidebar document using visual lines
    let sidebar_doc = build_sidebar_doc(&visual_lines, layout, page_height_pt, fonts)?;

    // 4. Build StripDocument with both content + sidebar
    let strip_doc = StripDocument::new(
        &document,
        &sidebar_doc,
        visual_lines,
        strip_height_pt,
        PPI,
    )?;
    info!("viewer: build_strip_document completed in {:.1}ms", start.elapsed().as_secs_f64() * 1000.0);
    Ok(strip_doc)
}

fn build_sidebar_doc(
    visual_lines: &[VisualLine],
    layout: &Layout,
    page_height_pt: f64,
    fonts: &FontCache,
) -> anyhow::Result<PagedDocument> {
    let sidebar_width_pt = layout.sidebar_cols as f64 * layout.cell_w as f64 * 72.0 / PPI as f64;
    let sidebar_source = generate_sidebar_typst(
        visual_lines,
        sidebar_width_pt,
        page_height_pt,
    );

    let sidebar_world = MluxWorld::new_raw(&sidebar_source, fonts);
    compile_document(&sidebar_world)
}
