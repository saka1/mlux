use std::time::Instant;

use anyhow::{Result, bail};
use log::info;
use typst::diag::{SourceDiagnostic, Severity, Tracepoint};
use typst::foundations::Smart;
use typst::layout::{Frame, FrameItem, Page, PagedDocument, Point};
use typst::visualize::Paint;
use typst::{World, WorldExt};

use crate::world::MluxWorld;

/// Format a SourceDiagnostic with source location, hints, and trace.
pub fn format_diagnostic(diag: &SourceDiagnostic, world: &MluxWorld<'_>) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let level = match diag.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };

    // Try to resolve source location and source line
    let location = diag.span.id()
        .and_then(|id| {
            let source = world.source(id).ok()?;
            let range = world.range(diag.span)?;
            let (line, col) = source.lines().byte_to_line_column(range.start)?;
            let source_line = source.text().lines().nth(line).map(String::from);
            Some((line + 1, col + 1, source_line)) // 0-indexed → 1-indexed
        });

    let _ = writeln!(out, "{level}: {}", diag.message);
    if let Some((line, col, source_line)) = &location {
        let _ = writeln!(out, "  --> main.typ:{line}:{col}");
        if let Some(src) = source_line {
            let _ = writeln!(out, "  {line:>4} | {src}");
        }
    }

    for hint in &diag.hints {
        let _ = writeln!(out, "  hint: {hint}");
    }

    for entry in &diag.trace {
        let trace_msg = match &entry.v {
            Tracepoint::Call(Some(name)) => format!("in call to '{name}'"),
            Tracepoint::Call(None) => "in call".to_string(),
            Tracepoint::Show(name) => format!("in show rule for '{name}'"),
            Tracepoint::Import => "in import".to_string(),
        };
        let _ = writeln!(out, "  trace: {trace_msg}");
    }

    out
}

/// Compile Typst sources into a PagedDocument (no rendering).
pub fn compile_document(world: &MluxWorld<'_>) -> Result<PagedDocument> {
    let start = Instant::now();
    let warned = typst::compile::<PagedDocument>(world);

    for warning in &warned.warnings {
        // テーマ (catppuccin.typ) はフォールバックチェーンとして複数フォントを
        // 宣言している (例: "IPAGothic", "Noto Sans CJK JP", "Noto Sans")。
        // システムにないフォールバックフォントごとに Typst が "unknown font family"
        // 警告を出すが、プライマリフォントが使えていれば実害はなく、毎回表示
        // されるだけのノイズになる。CJK フォントが一切ない場合の警告は
        // world.rs の FontCache::new() が別途出す。
        if warning.message.as_str().contains("unknown font family") {
            log::debug!("suppressed typst warning: {}", warning.message);
            continue;
        }
        eprint!("{}", format_diagnostic(warning, world));
    }

    match warned.output {
        Ok(doc) => {
            info!("render: typst::compile completed in {:.1}ms", start.elapsed().as_secs_f64() * 1000.0);
            Ok(doc)
        }
        Err(errors) => {
            let mut detail = String::new();
            for err in &errors {
                detail.push_str(&format_diagnostic(err, world));
            }
            bail!(
                "[BUG] typst compilation failed — this is a bug in mlux, not your input\n\
                 {} error(s):\n{detail}",
                errors.len()
            );
        }
    }
}

/// Render a single Frame to PNG bytes (used for strip-based rendering).
///
/// Wraps the frame in a Page, renders at the given PPI, and encodes to PNG.
pub fn render_frame_to_png(frame: &Frame, fill: &Smart<Option<Paint>>, ppi: f32) -> Result<Vec<u8>> {
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
