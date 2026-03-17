use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Instant;

use anyhow::{Result, bail};
use log::{debug, info, trace};
use serde::{Deserialize, Serialize};
use typst::foundations::Smart;
use typst::layout::{Abs, Axes, Frame, FrameItem, PagedDocument, Point};
use typst::syntax::Source;
use typst::visualize::{Geometry, Paint};

use crate::pipeline::ContentIndex;
use crate::tile_cache::TilePngs;
use crate::visual_line::{VisualLine, pt_to_px};

// ---------------------------------------------------------------------------
// Frame splitting — split a tall frame into vertical tiles
// ---------------------------------------------------------------------------

/// Estimate the bounding height of a FrameItem in pt.
fn item_bounding_height(item: &FrameItem) -> f64 {
    match item {
        FrameItem::Group(g) => g.frame.size().y.to_pt(),
        FrameItem::Text(t) => t.size.to_pt(),
        FrameItem::Shape(shape, _) => match &shape.geometry {
            Geometry::Line(p) => p.y.to_pt().abs(),
            Geometry::Rect(size) => size.y.to_pt(),
            // Conservative: use curve bounding box height (max - min Y)
            Geometry::Curve(curve) => {
                let mut min_y = f64::MAX;
                let mut max_y = f64::MIN;
                for item in curve.0.iter() {
                    let y = match item {
                        typst::visualize::CurveItem::Move(p) => p.y.to_pt(),
                        typst::visualize::CurveItem::Line(p) => p.y.to_pt(),
                        typst::visualize::CurveItem::Cubic(p1, p2, p3) => {
                            // Take max of all control/end points
                            let ys = [p1.y.to_pt(), p2.y.to_pt(), p3.y.to_pt()];
                            min_y = min_y.min(ys[0]).min(ys[1]).min(ys[2]);
                            ys.into_iter()
                                .max_by(|a, b| a.partial_cmp(b).unwrap())
                                .unwrap()
                        }
                        typst::visualize::CurveItem::Close => continue,
                    };
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
                if max_y > min_y { max_y - min_y } else { 0.0 }
            }
        },
        FrameItem::Image(_, size, _) => size.y.to_pt(),
        FrameItem::Link(_, size) => size.y.to_pt(),
        FrameItem::Tag(_) => 0.0,
    }
}

/// Split a compiled Frame into vertical tiles of `tile_height_pt` each.
///
/// Items spanning a tile boundary are cloned into both tiles.
/// tiny-skia clips drawing outside the canvas, so the visual result is correct.
pub fn split_frame(frame: &Frame, tile_height_pt: f64) -> Vec<Frame> {
    let start = Instant::now();
    let total_height = frame.size().y.to_pt();
    let tile_count = (total_height / tile_height_pt).ceil().max(1.0) as usize;
    let orig_width = frame.size().x;

    let mut tiles = Vec::with_capacity(tile_count);

    for i in 0..tile_count {
        let y_start = i as f64 * tile_height_pt;
        let y_end = ((i + 1) as f64 * tile_height_pt).min(total_height);
        let tile_h = y_end - y_start;

        let mut sub = Frame::hard(Axes {
            x: orig_width,
            y: Abs::pt(tile_h),
        });

        let mut item_count = 0u32;
        let mut spanning_count = 0u32;

        for (pos, item) in frame.items() {
            let item_y = pos.y.to_pt();
            let item_h = item_bounding_height(item);
            let item_bottom = item_y + item_h;

            // Does item overlap [y_start, y_end)?
            if item_bottom > y_start && item_y < y_end {
                let new_pos = Point::new(pos.x, Abs::pt(item_y - y_start));
                sub.push(new_pos, item.clone());
                item_count += 1;

                // Check if item spans beyond this tile
                if item_y < y_start || item_bottom > y_end {
                    spanning_count += 1;
                }
            }
        }

        debug!(
            "tile {}: {} items, {} boundary-spanning",
            i, item_count, spanning_count
        );
        tiles.push(sub);
    }

    info!(
        "tile: split_frame completed in {:.1}ms ({} tiles, height={}pt)",
        start.elapsed().as_secs_f64() * 1000.0,
        tile_count,
        tile_height_pt
    );
    tiles
}

// ---------------------------------------------------------------------------
// TileHash — content-addressed tile identity
// ---------------------------------------------------------------------------

/// 64-bit content hash identifying a tile's visual content.
///
/// Uses `Frame`'s derive `Hash` (which covers all fields including Span).
/// Same source text + same `FileId` produces identical Spans, so the hash
/// is deterministic across recompilations of the same input.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TileHash(u64);

#[cfg(test)]
impl TileHash {
    /// Create a TileHash from a raw value (test-only).
    pub(crate) fn new_for_test(v: u64) -> Self {
        Self(v)
    }
}

/// Combined hash of content + sidebar tiles. Both must match for cache reuse.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TilePairHash {
    pub content: TileHash,
    pub sidebar: TileHash,
}

/// Hash a Frame using its derive `Hash` impl.
pub fn compute_tile_hash(frame: &Frame) -> TileHash {
    let mut h = DefaultHasher::new();
    frame.hash(&mut h);
    TileHash(h.finish())
}

// ---------------------------------------------------------------------------
// TiledDocument — lazy tile-based rendering
// ---------------------------------------------------------------------------

/// Which tiles are visible for a given scroll position.
#[derive(Debug)]
pub enum VisibleTiles {
    /// Viewport fits entirely within one tile.
    Single { idx: usize, src_y: u32, src_h: u32 },
    /// Viewport straddles two tiles.
    Split {
        top_idx: usize,
        top_src_y: u32,
        top_src_h: u32,
        bot_idx: usize,
        bot_src_h: u32,
    },
}

/// Pure metadata extracted from a [`TiledDocument`].
///
/// Contains all information needed for layout, scrolling, and visual line queries
/// without requiring access to the renderable tile data. Serializable for IPC.
#[derive(Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub tile_count: usize,
    pub width_px: u32,
    pub sidebar_width_px: u32,
    pub tile_height_px: u32,
    pub total_height_px: u32,
    pub page_height_pt: f64,
    pub visual_lines: Vec<VisualLine>,
    /// Per-tile content hashes (tile index order). Empty if not computed.
    #[serde(default)]
    pub tile_hashes: Vec<TilePairHash>,
    pub content_index: ContentIndex,
    pub content_offset: usize,
}

impl DocumentMeta {
    /// Determine which tile(s) are visible at a given scroll offset.
    pub fn visible_tiles(&self, global_y: u32, vp_h: u32) -> VisibleTiles {
        let top_tile = (global_y / self.tile_height_px) as usize;
        let top_tile = top_tile.min(self.tile_count.saturating_sub(1));
        let src_y_in_tile = global_y - (top_tile as u32 * self.tile_height_px);
        // For the last tile, actual height may be less than tile_height_px.
        // Use total_height_px to derive actual height of each tile.
        let tile_actual_h = self.tile_actual_height_px(top_tile);
        let remaining_in_top = tile_actual_h.saturating_sub(src_y_in_tile);

        if remaining_in_top >= vp_h || top_tile + 1 >= self.tile_count {
            let src_h = vp_h.min(remaining_in_top);
            debug!(
                "display: single tile {}, src_y={}, src_h={}, vp_h={}",
                top_tile, src_y_in_tile, src_h, vp_h
            );
            VisibleTiles::Single {
                idx: top_tile,
                src_y: src_y_in_tile,
                src_h,
            }
        } else {
            let top_src_h = remaining_in_top;
            let bot_idx = top_tile + 1;
            let bot_src_h = (vp_h - top_src_h).min(self.tile_actual_height_px(bot_idx));
            debug!(
                "display: split tiles [{}, {}], top_src_y={}, top_h={}, bot_h={}, vp_h={}",
                top_tile, bot_idx, src_y_in_tile, top_src_h, bot_src_h, vp_h
            );
            VisibleTiles::Split {
                top_idx: top_tile,
                top_src_y: src_y_in_tile,
                top_src_h,
                bot_idx,
                bot_src_h,
            }
        }
    }

    /// Maximum scroll offset.
    pub fn max_scroll(&self, vp_h: u32) -> u32 {
        self.total_height_px.saturating_sub(vp_h)
    }

    /// Actual pixel height of a specific tile (last tile may be shorter).
    fn tile_actual_height_px(&self, idx: usize) -> u32 {
        if idx + 1 < self.tile_count {
            self.tile_height_px
        } else {
            // Last tile: remaining height
            self.total_height_px
                .saturating_sub(idx as u32 * self.tile_height_px)
        }
    }
}

/// Bundled content mapping data passed to [`TiledDocument::new`].
pub struct ContentMapping {
    pub source: Source,
    pub content_index: ContentIndex,
    pub content_offset: usize,
}

/// A document split into renderable tiles for lazy, bounded-memory rendering.
///
/// All methods take `&self` — rendering is pure (no internal caching).
/// Use [`crate::tile_cache::TileCache`] separately for caching rendered PNGs.
pub struct TiledDocument {
    tiles: Vec<Frame>,
    sidebar_tiles: Vec<Frame>,
    sidebar_fill: Smart<Option<Paint>>,
    page_fill: Smart<Option<Paint>>,
    ppi: f32,
    width_px: u32,
    sidebar_width_px: u32,
    tile_height_px: u32,
    total_height_px: u32,
    page_height_pt: f64,
    visual_lines: Vec<VisualLine>,
    source: Source,
    content_index: ContentIndex,
    content_offset: usize,
}

impl TiledDocument {
    /// Build a TiledDocument from a compiled content + sidebar PagedDocument.
    ///
    /// - `sidebar_doc`: compiled sidebar document (same page height as content)
    /// - `visual_lines`: pre-extracted visual line positions (avoids redundant extraction)
    /// - `tile_height_pt`: height of each tile in typst points
    /// - `ppi`: rendering resolution
    pub fn new(
        document: &PagedDocument,
        sidebar_doc: &PagedDocument,
        visual_lines: Vec<VisualLine>,
        tile_height_pt: f64,
        ppi: f32,
        content_mapping: ContentMapping,
    ) -> Result<Self> {
        if document.pages.is_empty() {
            bail!("[BUG] document has no pages");
        }
        let page = &document.pages[0];

        let page_size = page.frame.size();
        info!(
            "compiled: {:.1}x{:.1}pt, {} top-level items",
            page_size.x.to_pt(),
            page_size.y.to_pt(),
            page.frame.items().count()
        );

        let tiles = split_frame(&page.frame, tile_height_pt);

        // Split sidebar with the same tile boundaries
        if sidebar_doc.pages.is_empty() {
            bail!("[BUG] sidebar document has no pages");
        }
        let sidebar_page = &sidebar_doc.pages[0];
        let sidebar_tiles = split_frame(&sidebar_page.frame, tile_height_pt);
        let sidebar_width_px = pt_to_px(sidebar_page.frame.size().x.to_pt(), ppi);
        info!(
            "sidebar: {} tiles, {}px wide",
            sidebar_tiles.len(),
            sidebar_width_px
        );

        let width_px = pt_to_px(page_size.x.to_pt(), ppi);
        let tile_height_px = pt_to_px(tile_height_pt, ppi);
        let total_height_px = pt_to_px(page_size.y.to_pt(), ppi);
        let page_height_pt = page_size.y.to_pt();

        Ok(Self {
            tiles,
            sidebar_tiles,
            sidebar_fill: sidebar_page.fill.clone(),
            page_fill: page.fill.clone(),
            ppi,
            width_px,
            sidebar_width_px,
            tile_height_px,
            total_height_px,
            page_height_pt,
            visual_lines,
            source: content_mapping.source,
            content_index: content_mapping.content_index,
            content_offset: content_mapping.content_offset,
        })
    }

    /// Render a single content tile to PNG bytes.
    ///
    /// This is a pure function -- no internal caching.
    /// Thread-safe: `Frame.items` uses `Arc`, `typst_render::render` is stateless.
    pub fn render_tile(&self, idx: usize) -> Result<Vec<u8>> {
        self.render_frame(idx, &self.tiles, &self.page_fill, "content")
    }

    /// Render a single sidebar tile to PNG bytes.
    pub fn render_sidebar_tile(&self, idx: usize) -> Result<Vec<u8>> {
        self.render_frame(idx, &self.sidebar_tiles, &self.sidebar_fill, "sidebar")
    }

    /// Render a frame from `tiles` at `idx` to PNG bytes.
    fn render_frame(
        &self,
        idx: usize,
        tiles: &[Frame],
        fill: &Smart<Option<Paint>>,
        label: &str,
    ) -> Result<Vec<u8>> {
        assert!(idx < tiles.len(), "{label} tile index out of bounds");
        trace!("rendering {label} tile {idx}");
        crate::pipeline::render_frame_to_png(&tiles[idx], fill, self.ppi)
    }

    /// Compute content hashes for all tile pairs.
    pub fn compute_tile_hashes(&self) -> Vec<TilePairHash> {
        let start = Instant::now();
        let hashes: Vec<TilePairHash> = self
            .tiles
            .iter()
            .zip(self.sidebar_tiles.iter())
            .map(|(content, sidebar)| TilePairHash {
                content: compute_tile_hash(content),
                sidebar: compute_tile_hash(sidebar),
            })
            .collect();
        info!(
            "tile: computed {} tile hashes in {:.1}ms",
            hashes.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        hashes
    }

    /// Extract pure metadata (no renderable data), including tile hashes.
    pub fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            tile_count: self.tiles.len(),
            width_px: self.width_px,
            sidebar_width_px: self.sidebar_width_px,
            tile_height_px: self.tile_height_px,
            total_height_px: self.total_height_px,
            page_height_pt: self.page_height_pt,
            visual_lines: self.visual_lines.clone(),
            tile_hashes: self.compute_tile_hashes(),
            content_index: self.content_index.clone(),
            content_offset: self.content_offset,
        }
    }

    /// Render both content and sidebar tiles for a given index.
    pub fn render_tile_pair(&self, idx: usize) -> Result<TilePngs> {
        let content = self.render_tile(idx)?;
        let sidebar = self.render_sidebar_tile(idx)?;
        Ok(TilePngs { content, sidebar })
    }

    /// Find highlight rectangles for a tile's content (no rendering).
    pub fn find_tile_highlight_rects(
        &self,
        idx: usize,
        spec: &crate::highlight::HighlightSpec,
    ) -> Vec<crate::highlight::HighlightRect> {
        crate::highlight::find_highlight_rects(&self.tiles[idx], spec, self.ppi, &self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::syntax::Span;

    // -----------------------------------------------------------------------
    // TileHash tests
    // -----------------------------------------------------------------------

    /// Create a simple Frame with a rect shape for hash testing.
    fn make_test_frame(width: f64, height: f64) -> Frame {
        let mut frame = Frame::hard(Axes::new(Abs::pt(width), Abs::pt(height)));
        let shape = typst::visualize::Shape {
            geometry: Geometry::Rect(Axes::new(Abs::pt(width), Abs::pt(height))),
            fill: None,
            fill_rule: Default::default(),
            stroke: None,
        };
        frame.push(Point::zero(), FrameItem::Shape(shape, Span::detached()));
        frame
    }

    #[test]
    fn compute_tile_hash_same_frame_same_hash() {
        let f1 = make_test_frame(100.0, 200.0);
        let f2 = make_test_frame(100.0, 200.0);
        assert_eq!(compute_tile_hash(&f1), compute_tile_hash(&f2));
    }

    #[test]
    fn compute_tile_hash_different_frame_different_hash() {
        let f1 = make_test_frame(100.0, 200.0);
        let f2 = make_test_frame(100.0, 201.0);
        assert_ne!(compute_tile_hash(&f1), compute_tile_hash(&f2));
    }

    #[test]
    fn compute_tile_hash_empty_frames() {
        let f1 = Frame::hard(Axes::new(Abs::pt(100.0), Abs::pt(100.0)));
        let f2 = Frame::hard(Axes::new(Abs::pt(100.0), Abs::pt(100.0)));
        assert_eq!(compute_tile_hash(&f1), compute_tile_hash(&f2));
    }
}
