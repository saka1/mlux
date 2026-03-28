use std::time::Instant;

use anyhow::Result;
use log::info;
use typst::foundations::Smart;
use typst::layout::{Frame, Page};
use typst::visualize::Paint;

/// Render a single Frame to PNG bytes (used for tile-based rendering).
///
/// Wraps the frame in a Page, renders at the given PPI, and encodes to PNG.
pub fn render_frame_to_png(
    frame: &Frame,
    fill: &Smart<Option<Paint>>,
    ppi: f32,
) -> Result<Vec<u8>> {
    let start = Instant::now();
    let page = Page {
        frame: frame.clone(),
        fill: fill.clone(),
        numbering: None,
        supplement: typst::foundations::Content::empty(),
        number: 0,
    };

    let pixel_per_pt = ppi / 72.0;
    let pixmap = typst_render::render(&page, pixel_per_pt);

    let png = pixmap
        .encode_png()
        .map_err(|e| anyhow::anyhow!("[BUG] PNG encoding failed: {e}"))?;
    info!(
        "render: render_frame_to_png completed in {:.1}ms ({}x{}px, {} bytes)",
        start.elapsed().as_secs_f64() * 1000.0,
        pixmap.width(),
        pixmap.height(),
        png.len()
    );
    Ok(png)
}
