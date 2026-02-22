use anyhow::{Result, bail};
use typst::layout::PagedDocument;

use crate::world::TmarkWorld;

/// Compile Typst sources and render to PNG bytes.
pub fn render_to_png(world: &TmarkWorld, ppi: f32) -> Result<Vec<u8>> {
    let warned = typst::compile::<PagedDocument>(world);

    // Print warnings to stderr
    for warning in &warned.warnings {
        eprintln!("typst warning: {}", warning.message);
    }

    let document = match warned.output {
        Ok(doc) => doc,
        Err(errors) => {
            for err in &errors {
                eprintln!("typst error: {}", err.message);
            }
            bail!("typst compilation failed with {} error(s)", errors.len());
        }
    };

    if document.pages.is_empty() {
        bail!("typst produced no pages");
    }

    let pixel_per_pt = ppi / 72.0;
    let pixmap = typst_render::render(&document.pages[0], pixel_per_pt);

    pixmap
        .encode_png()
        .map_err(|e| anyhow::anyhow!("PNG encoding failed: {e}"))
}
