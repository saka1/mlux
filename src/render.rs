use std::fmt::Write as _;

use anyhow::{Result, bail};
use typst::layout::{Frame, FrameItem, PagedDocument, Point};

use crate::world::TmarkWorld;

/// A visual line extracted from the PagedDocument frame tree.
#[derive(Debug, Clone)]
pub struct VisualLine {
    pub y_pt: f64, // Absolute Y coordinate of the text baseline (pt)
    pub y_px: u32, // Pixel Y coordinate (after ppi conversion)
}

/// Compile Typst sources into a PagedDocument (no rendering).
pub fn compile_document(world: &TmarkWorld) -> Result<PagedDocument> {
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

/// Extract visual line positions from the frame tree.
///
/// Walks all TextItem nodes, collects their absolute Y coordinates,
/// deduplicates with 0.5pt tolerance, and returns sorted VisualLines.
pub fn extract_visual_lines(document: &PagedDocument, ppi: f32) -> Vec<VisualLine> {
    if document.pages.is_empty() {
        return Vec::new();
    }

    let mut y_coords: Vec<f64> = Vec::new();
    collect_text_y(&document.pages[0].frame, Point::zero(), &mut y_coords);

    // Sort and deduplicate.
    //
    // Tolerance is 5pt: within a single visual line, different font sizes
    // (e.g., 12pt body vs 10pt inline code) produce baseline offsets of
    // up to ~2.6pt (0.59pt font metric diff + 2pt inset). The minimum
    // inter-line gap is ~15pt (heading → body), so 5pt safely merges
    // intra-line variants without collapsing separate lines.
    const TOLERANCE_PT: f64 = 5.0;

    y_coords.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut deduped: Vec<f64> = Vec::new();
    for y in y_coords {
        if deduped.last().map_or(true, |prev| (y - prev).abs() > TOLERANCE_PT) {
            deduped.push(y);
        }
    }

    let pixel_per_pt = ppi as f64 / 72.0;
    deduped
        .into_iter()
        .map(|y_pt| VisualLine {
            y_pt,
            y_px: (y_pt * pixel_per_pt).round() as u32,
        })
        .collect()
}

/// Recursively collect absolute Y coordinates of all TextItem nodes.
fn collect_text_y(frame: &Frame, parent_offset: Point, out: &mut Vec<f64>) {
    for (pos, item) in frame.items() {
        let abs = parent_offset + *pos;
        match item {
            FrameItem::Text(_) => {
                out.push(abs.y.to_pt());
            }
            FrameItem::Group(group) => {
                collect_text_y(&group.frame, abs, out);
            }
            _ => {}
        }
    }
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

/// Compile Typst sources and render to PNG bytes (convenience wrapper).
pub fn render_to_png(world: &TmarkWorld, ppi: f32) -> Result<Vec<u8>> {
    let document = compile_document(world)?;
    render_page_to_png(&document, ppi)
}
