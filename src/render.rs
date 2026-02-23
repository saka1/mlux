use anyhow::{Result, bail};
use typst::layout::{Frame, FrameItem, PagedDocument, Point};

use crate::world::MluxWorld;

/// Compile Typst sources into a PagedDocument (no rendering).
pub fn compile_document(world: &MluxWorld) -> Result<PagedDocument> {
    let warned = typst::compile::<PagedDocument>(world);

    for warning in &warned.warnings {
        eprintln!("typst warning: {}", warning.message);
    }

    match warned.output {
        Ok(doc) => Ok(doc),
        Err(errors) => {
            for err in &errors {
                eprintln!("typst error: {}", err.message);
            }
            bail!("typst compilation failed with {} error(s)", errors.len());
        }
    }
}

/// Render a PagedDocument's first page to PNG bytes.
pub fn render_page_to_png(document: &PagedDocument, ppi: f32) -> Result<Vec<u8>> {
    if document.pages.is_empty() {
        bail!("typst produced no pages");
    }

    let pixel_per_pt = ppi / 72.0;
    let pixmap = typst_render::render(&document.pages[0], pixel_per_pt);

    pixmap
        .encode_png()
        .map_err(|e| anyhow::anyhow!("PNG encoding failed: {e}"))
}

/// Compile Typst sources and render to PNG bytes (convenience wrapper).
pub fn render_to_png(world: &MluxWorld, ppi: f32) -> Result<Vec<u8>> {
    let document = compile_document(world)?;
    render_page_to_png(&document, ppi)
}

/// Dump the PagedDocument frame tree to stderr for debugging.
pub fn dump_document(document: &PagedDocument) {
    eprintln!("=== PagedDocument: {} page(s) ===", document.pages.len());
    for (i, page) in document.pages.iter().enumerate() {
        let s = page.frame.size();
        eprintln!("Page {i}: {:.1}pt x {:.1}pt", s.x.to_pt(), s.y.to_pt());
        dump_frame(&page.frame, 0, Point::zero());
    }
}

fn dump_frame(frame: &Frame, depth: usize, parent_offset: Point) {
    let indent = "  ".repeat(depth);
    for (pos, item) in frame.items() {
        let abs_x = (parent_offset.x + pos.x).to_pt();
        let abs_y = (parent_offset.y + pos.y).to_pt();
        match item {
            FrameItem::Text(text) => {
                let preview: String = text.text.chars().take(40).collect();
                eprintln!(
                    "{indent}Text  ({abs_x:.1}, {abs_y:.1})pt  size={:.1}pt  glyphs={}  {:?}",
                    text.size.to_pt(),
                    text.glyphs.len(),
                    preview,
                );
            }
            FrameItem::Group(group) => {
                let s = group.frame.size();
                eprintln!(
                    "{indent}Group ({abs_x:.1}, {abs_y:.1})pt  {:.1}x{:.1}pt",
                    s.x.to_pt(),
                    s.y.to_pt(),
                );
                dump_frame(&group.frame, depth + 1, parent_offset + *pos);
            }
            FrameItem::Shape(_, _) => {
                eprintln!("{indent}Shape ({abs_x:.1}, {abs_y:.1})pt");
            }
            FrameItem::Image(_, size, _) => {
                eprintln!(
                    "{indent}Image ({abs_x:.1}, {abs_y:.1})pt  {:.1}x{:.1}pt",
                    size.x.to_pt(),
                    size.y.to_pt(),
                );
            }
            FrameItem::Link(_, size) => {
                eprintln!(
                    "{indent}Link  ({abs_x:.1}, {abs_y:.1})pt  {:.1}x{:.1}pt",
                    size.x.to_pt(),
                    size.y.to_pt(),
                );
            }
            FrameItem::Tag(_) => {
                eprintln!("{indent}Tag   ({abs_x:.1}, {abs_y:.1})pt");
            }
        }
    }
}
