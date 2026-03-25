use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, bail};
use log::info;
use typst::layout::PagedDocument;

use super::content_index::{BoundIndex, ContentIndex};
use super::world::{FontCache, MluxWorld};
use crate::tile::{ContentMapping, TiledDocument};
use crate::visual_line::{VisualLine, extract_visual_lines_with_map};

/// Parameters for [`build_tiled_document`].
#[derive(Clone)]
pub struct BuildParams {
    pub theme_spec: String,
    pub detected_light: bool,
    pub markdown: String,
    pub base_dir: Option<PathBuf>,
    pub width_pt: f64,
    pub sidebar_width_pt: f64,
    pub tile_height_pt: f64,
    pub ppi: f32,
    pub fonts: &'static FontCache,
    pub allow_remote_images: bool,
}

/// Result of the shared compilation pipeline (steps 1-4).
struct CompiledContent {
    theme_name: String,
    world: MluxWorld,
    document: PagedDocument,
    content_index: ContentIndex,
}

/// Shared build steps: theme resolution, diagram rendering,
/// markdown→typst, world construction, and compilation.
///
/// Prescan and image loading are the caller's responsibility.
fn compile_content(
    params: &BuildParams,
    prescan: &super::markup::Prescan,
    mut image_files: crate::image::LoadedImages,
) -> Result<CompiledContent> {
    // 0. Theme resolution (from prescan CJK detection)
    let theme_name = crate::theme::resolve_theme_name(
        &params.theme_spec,
        params.detected_light,
        prescan.has_cjk,
    );
    info!(
        "document: {}, theme: {theme_name}",
        if prescan.has_cjk { "CJK" } else { "latin" }
    );
    let theme_text = crate::theme::get(theme_name)
        .ok_or_else(|| anyhow::anyhow!("unknown theme '{theme_name}'"))?;
    let data_files = crate::theme::data_files(theme_name);

    // 1. Diagram pipeline
    let diagrams = crate::diagram::extract_diagrams(&params.markdown);
    let mermaid_colors = crate::theme::mermaid_colors(theme_name);
    for (key, svg) in crate::diagram::render_diagrams(&diagrams, mermaid_colors) {
        image_files.insert(key, svg);
    }

    // 2. Markdown -> Typst
    let loaded_set = image_files.key_set();
    let (content_text, content_index) =
        super::markup::markdown_to_typst(&params.markdown, Some(&loaded_set));

    // 3. Compile content document
    let world = MluxWorld::new(
        theme_text,
        data_files,
        &content_text,
        params.width_pt,
        params.fonts,
        image_files,
    );
    let document = super::typst_compile::compile_document(&world)?;

    Ok(CompiledContent {
        theme_name: theme_name.to_string(),
        world,
        document,
        content_index,
    })
}

/// Compile from pre-loaded images and dump the generated Typst source and frame tree to stderr.
pub(crate) fn compile_and_dump(
    params: &BuildParams,
    prescan: &super::markup::Prescan,
    image_files: crate::image::LoadedImages,
) -> Result<()> {
    let compiled = compile_content(params, prescan, image_files)?;

    // Print generated main.typ to stderr
    let source_text = compiled.world.main_source().text();
    eprintln!(
        "=== Generated main.typ ({} lines) ===",
        source_text.lines().count()
    );
    for (i, line) in source_text.lines().enumerate() {
        eprintln!("{:>4} | {}", i + 1, line);
    }
    eprintln!();

    super::typst_compile::dump_document(&compiled.document);
    Ok(())
}

/// Build a TiledDocument from Markdown source (test-only convenience).
///
/// Convenience wrapper that loads images internally then delegates to
/// [`compile_and_tile`]. Production code uses [`crate::renderer::build_renderer`] instead.
pub fn build_tiled_document(params: &BuildParams) -> Result<TiledDocument> {
    let prescan = super::markup::prescan(&params.markdown);
    let (images, errors) = crate::image::load_images(
        &prescan.image_paths,
        params.base_dir.as_deref(),
        params.allow_remote_images,
    );
    for err in &errors {
        log::warn!("{err}");
    }
    compile_and_tile(params, &prescan, images)
}

/// Compile from pre-loaded images and build a TiledDocument.
///
/// Core pipeline: diagram rendering, Markdown→Typst conversion,
/// Typst compilation, visual line extraction, sidebar generation,
/// and tile assembly.
///
/// Called from the forked child process in [`crate::renderer`].
pub(crate) fn compile_and_tile(
    params: &BuildParams,
    prescan: &super::markup::Prescan,
    image_files: crate::image::LoadedImages,
) -> Result<TiledDocument> {
    let start = Instant::now();

    let CompiledContent {
        theme_name,
        world: content_world,
        document,
        content_index,
    } = compile_content(params, prescan, image_files)?;

    // 4. Extract visual lines with source mapping
    let bound_index = BoundIndex::new(
        &content_index,
        content_world.main_source(),
        content_world.content_offset(),
        &params.markdown,
    );
    let visual_lines = extract_visual_lines_with_map(&document, params.ppi, Some(&bound_index));

    if document.pages.is_empty() {
        bail!("[BUG] document has no pages");
    }
    let page_height_pt = document.pages[0].frame.size().y.to_pt();

    // 6. Compile sidebar document using visual lines
    let sidebar_source = generate_sidebar_typst(
        &visual_lines,
        params.sidebar_width_pt,
        page_height_pt,
        &theme_name,
    );
    let sidebar_world = MluxWorld::new_raw(&sidebar_source, params.fonts);
    let sidebar_doc = super::typst_compile::compile_document(&sidebar_world)?;

    // 7. Build TiledDocument with both content + sidebar
    let tiled_doc = TiledDocument::new(
        &document,
        &sidebar_doc,
        visual_lines,
        params.tile_height_pt,
        params.ppi,
        ContentMapping {
            source: content_world.main_source().clone(),
            content_index,
            content_offset: content_world.content_offset(),
        },
    )?;
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
    theme_name: &str,
) -> String {
    let (bg, fg) = crate::theme::sidebar_colors(theme_name);
    let mut src = String::new();
    writeln!(
        src,
        "#set page(width: {sidebar_width_pt}pt, height: {page_height_pt}pt, margin: 0pt, fill: rgb(\"{bg}\"))"
    )
    .unwrap();
    writeln!(
        src,
        "#set text(font: \"Fira Mono\", size: 8pt, fill: rgb(\"{fg}\"))"
    )
    .unwrap();

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        let dy = line.y_pt;
        // Place at baseline Y; use top+right alignment with ascent offset.
        // 8pt text has ~6pt ascent, so shift up to align baselines.
        writeln!(
            src,
            "#place(top + right, dy: {dy}pt - 6pt, dx: -4pt)[#text(size: 8pt)[{line_num}]]"
        )
        .unwrap();
    }

    src
}
