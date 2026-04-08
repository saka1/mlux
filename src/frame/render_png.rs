use std::time::Instant;

use anyhow::Result;
use log::info;
use typst::foundations::Smart;
use typst::layout::{Frame, Page};
use typst::visualize::Paint;

/// Render a single Frame to PNG bytes (used for tile-based rendering).
///
/// Wraps the frame in a Page, renders at the given PPI, and encodes to PNG.
/// When `fast` is true, uses minimal compression for lower latency at the cost
/// of slightly larger output.  The `render` subcommand passes `false` to keep
/// files small.
pub fn render_frame_to_png(
    frame: &Frame,
    fill: &Smart<Option<Paint>>,
    ppi: f32,
    fast: bool,
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
    let mut pixmap = typst_render::render(&page, pixel_per_pt);

    // Demultiply alpha in-place.  tiny-skia stores premultiplied RGBA;
    // the PNG format requires straight alpha.  Modifying the owned pixmap
    // directly avoids the full-pixmap clone that Pixmap::encode_png() does.
    demultiply_alpha(pixmap.data_mut());

    let compression = if fast {
        png::Compression::Fastest
    } else {
        png::Compression::Fast
    };

    let mut data = Vec::with_capacity(pixmap.data().len() / 2);
    {
        let mut encoder = png::Encoder::new(&mut data, pixmap.width(), pixmap.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(compression);
        let mut writer = encoder
            .write_header()
            .map_err(|e| anyhow::anyhow!("[BUG] PNG header write failed: {e}"))?;
        writer
            .write_image_data(pixmap.data())
            .map_err(|e| anyhow::anyhow!("[BUG] PNG encoding failed: {e}"))?;
    }

    info!(
        "render: render_frame_to_png completed in {:.1}ms ({}x{}px, {} bytes, {})",
        start.elapsed().as_secs_f64() * 1000.0,
        pixmap.width(),
        pixmap.height(),
        data.len(),
        if fast { "fast" } else { "default" },
    );
    Ok(data)
}

/// Convert premultiplied RGBA to straight alpha, in place.
///
/// The `+ 0.5` rounding can produce 256.0 for near-opaque pixels (e.g. R=255,
/// A=254).  Rust's `f64 as u8` saturates to 255, which is the correct result.
fn demultiply_alpha(data: &mut [u8]) {
    for pixel in data.chunks_exact_mut(4) {
        let a = pixel[3];
        if a == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
        } else if a < 255 {
            let a_f = a as f64 / 255.0;
            pixel[0] = (pixel[0] as f64 / a_f + 0.5) as u8;
            pixel[1] = (pixel[1] as f64 / a_f + 0.5) as u8;
            pixel[2] = (pixel[2] as f64 / a_f + 0.5) as u8;
        }
    }
}
