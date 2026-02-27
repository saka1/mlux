use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::info;

use mlux::convert::markdown_to_typst_with_map;
use mlux::render::{compile_document, dump_document};
use mlux::strip::{DEFAULT_SIDEBAR_WIDTH_PT, build_strip_document};
use mlux::world::{FontCache, MluxWorld};

#[derive(Parser)]
#[command(name = "mlux", about = "Markdown viewer and renderer powered by Typst")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Input Markdown file (for view mode)
    #[arg(global = true)]
    input: Option<PathBuf>,

    /// Theme name (loaded from themes/{name}.typ)
    #[arg(long, default_value = "catppuccin", global = true)]
    theme: String,
}

#[derive(Subcommand)]
enum Command {
    /// Render Markdown to PNG
    Render {
        /// Input Markdown file
        input: PathBuf,

        /// Output PNG file
        #[arg(short, long, default_value = "output.png")]
        output: PathBuf,

        /// Page width in pt
        #[arg(long, default_value_t = 660.0)]
        width: f64,

        /// Output resolution in PPI
        #[arg(long, default_value_t = 144.0)]
        ppi: f32,

        /// Strip height in pt
        #[arg(long, default_value_t = 500.0)]
        strip_height: f64,

        /// Dump frame tree to stderr
        #[arg(long)]
        dump: bool,
    },
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::Render { input, output, width, ppi, strip_height, dump }) => {
            cmd_render(input, cli.theme, output, width, ppi, strip_height, dump)
        }
        None => {
            let input = match cli.input {
                Some(p) => p,
                None => {
                    eprintln!("Error: input file required");
                    std::process::exit(1);
                }
            };
            mlux::viewer::run(input, cli.theme)
        }
    };

    if let Err(e) = result {
        let msg = format!("{e:#}");
        if msg.contains("[BUG]") {
            eprintln!("\x1b[1;31m{msg}\x1b[0m");
        } else {
            eprintln!("Error: {msg}");
        }
        std::process::exit(1);
    }
}

fn cmd_render(
    input: PathBuf,
    theme: String,
    output: PathBuf,
    width: f64,
    ppi: f32,
    strip_height: f64,
    dump: bool,
) -> Result<()> {
    let pipeline_start = Instant::now();

    // Read input markdown
    let markdown = fs::read_to_string(&input)
        .with_context(|| format!("failed to read {}", input.display()))?;

    // Read theme file
    let theme_path = PathBuf::from(format!("themes/{}.typ", theme));
    let theme_text = fs::read_to_string(&theme_path)
        .with_context(|| format!("failed to read theme {}", theme_path.display()))?;

    if markdown.trim().is_empty() {
        anyhow::bail!("input file is empty or contains only whitespace");
    }

    // Convert markdown to typst
    let (content_text, source_map) = markdown_to_typst_with_map(&markdown);

    // Create font cache (one-time filesystem scan)
    let font_cache = FontCache::new();

    if dump {
        // Dump mode: build world directly for source inspection
        let world = MluxWorld::new(&theme_text, &content_text, width, &font_cache);
        let source_text = world.main_source().text();
        eprintln!("=== Generated main.typ ({} lines) ===", source_text.lines().count());
        for (i, line) in source_text.lines().enumerate() {
            eprintln!("{:>4} | {}", i + 1, line);
        }
        eprintln!();

        match compile_document(&world) {
            Ok(doc) => dump_document(&doc),
            Err(e) => eprintln!("{e:#}"),
        }
        return Ok(());
    }

    let strip_doc = build_strip_document(
        &theme_text, &content_text, &markdown, &source_map,
        width, DEFAULT_SIDEBAR_WIDTH_PT, strip_height, ppi, &font_cache,
    )?;

    let stem = output.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = output.extension().unwrap_or_default().to_string_lossy().to_string();
    let parent = output.parent().unwrap_or_else(|| std::path::Path::new("."));
    fs::create_dir_all(parent).ok();

    let mut files = Vec::new();
    for i in 0..strip_doc.strip_count() {
        let png_data = strip_doc.render_strip(i)?;
        let filename = format!("{}-{:03}.{}", stem, i, ext);
        let path = parent.join(&filename);
        fs::write(&path, &png_data)
            .with_context(|| format!("failed to write {}", path.display()))?;
        files.push((filename, png_data.len()));
    }

    info!(
        "cmd_render: total pipeline completed in {:.1}ms",
        pipeline_start.elapsed().as_secs_f64() * 1000.0
    );

    eprintln!("rendered {} -> {} strip(s):", input.display(), strip_doc.strip_count());
    for (filename, size) in &files {
        eprintln!("  {} ({} bytes)", filename, size);
    }

    Ok(())
}
