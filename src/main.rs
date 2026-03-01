use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::info;

use mlux::config;
use mlux::convert::markdown_to_typst_with_map;
use mlux::input::{self, InputSource};
use mlux::render::{compile_document, dump_document};
use mlux::tile::{BuildParams, DEFAULT_SIDEBAR_WIDTH_PT, build_tiled_document};
use mlux::world::{FontCache, MluxWorld};

#[derive(Parser)]
#[command(name = "mlux", about = "Markdown viewer and renderer powered by Typst")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Input Markdown file (for view mode; use `-` for stdin)
    #[arg(global = true)]
    input: Option<PathBuf>,

    /// Theme name (loaded from themes/{name}.typ)
    #[arg(long, global = true)]
    theme: Option<String>,

    /// Disable automatic file watching (viewer reloads on file change by default)
    #[arg(long, global = true)]
    no_watch: bool,

    /// Log output file path (enables logging when specified)
    #[arg(long, global = true)]
    log: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Render Markdown to PNG
    Render {
        /// Input Markdown file (use `-` for stdin)
        input: PathBuf,

        /// Output PNG file
        #[arg(short, long, default_value = "output.png")]
        output: PathBuf,

        /// Page width in pt
        #[arg(long)]
        width: Option<f64>,

        /// Output resolution in PPI
        #[arg(long)]
        ppi: Option<f32>,

        /// Tile height in pt
        #[arg(long)]
        tile_height: Option<f64>,

        /// Dump frame tree to stderr
        #[arg(long)]
        dump: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Some(log_path) = &cli.log {
        let file = std::fs::File::create(log_path).expect("failed to open log file");
        env_logger::Builder::from_default_env()
            .target(env_logger::Target::Pipe(Box::new(file)))
            .init();
    } else if cli.command.is_some() {
        env_logger::init();
    }
    // viewer mode + no --log → logger not initialized (no log output)

    // Load config file and merge CLI overrides
    let mut cfg = match config::load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    };

    // Extract render-subcommand CLI overrides (width/ppi/tile_height)
    let (render_width, render_ppi, render_tile_height) = match &cli.command {
        Some(Command::Render {
            width,
            ppi,
            tile_height,
            ..
        }) => (*width, *ppi, *tile_height),
        None => (None, None, None),
    };

    // Build CliOverrides for viewer config reload support
    let cli_overrides = config::CliOverrides {
        theme: cli.theme.clone(),
        width: render_width,
        ppi: render_ppi,
        tile_height: render_tile_height,
    };

    cfg.merge_cli(cli.theme, render_width, render_ppi, render_tile_height);

    let config = cfg.resolve();

    let result = match cli.command {
        Some(Command::Render {
            input,
            output,
            dump,
            ..
        }) => cmd_render(input, &config, output, dump),
        None => {
            let input_source = if input::is_stdin_input(cli.input.as_deref()) {
                InputSource::Stdin(input::StdinReader::new())
            } else {
                match cli.input {
                    Some(p) => InputSource::File(p),
                    None => {
                        eprintln!("Error: input file required (or pipe via stdin)");
                        std::process::exit(1);
                    }
                }
            };
            mlux::viewer::run(input_source, config, &cli_overrides, !cli.no_watch)
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

fn cmd_render(input: PathBuf, config: &config::Config, output: PathBuf, dump: bool) -> Result<()> {
    let pipeline_start = Instant::now();

    let width = config.width;
    let ppi = config.ppi;
    let tile_height = config.viewer.tile_height;
    let theme = &config.theme;

    // Read input markdown (support `-` for stdin)
    let is_stdin = input.as_os_str() == "-";
    let markdown = if is_stdin {
        input::read_stdin_to_string().context("failed to read stdin")?
    } else {
        fs::read_to_string(&input).with_context(|| format!("failed to read {}", input.display()))?
    };

    // Look up built-in theme
    let theme_text =
        mlux::theme::get(theme).ok_or_else(|| anyhow::anyhow!("unknown theme '{theme}'"))?;

    if markdown.trim().is_empty() {
        anyhow::bail!("input file is empty or contains only whitespace");
    }

    // Convert markdown to typst
    let (content_text, source_map) = markdown_to_typst_with_map(&markdown);

    // Create font cache (one-time filesystem scan)
    let font_cache = FontCache::new();

    if dump {
        // Dump mode: build world directly for source inspection
        let world = MluxWorld::new(theme_text, &content_text, width, &font_cache);
        let source_text = world.main_source().text();
        eprintln!(
            "=== Generated main.typ ({} lines) ===",
            source_text.lines().count()
        );
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

    let tiled_doc = build_tiled_document(&BuildParams {
        theme_text,
        content_text: &content_text,
        md_source: &markdown,
        source_map: &source_map,
        width_pt: width,
        sidebar_width_pt: DEFAULT_SIDEBAR_WIDTH_PT,
        tile_height_pt: tile_height,
        ppi,
        fonts: &font_cache,
    })?;

    let stem = output
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let ext = output
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = output.parent().unwrap_or_else(|| std::path::Path::new("."));
    fs::create_dir_all(parent).ok();

    let mut files = Vec::new();
    for i in 0..tiled_doc.tile_count() {
        let png_data = tiled_doc.render_tile(i)?;
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

    let input_name = if is_stdin {
        "<stdin>".to_string()
    } else {
        input.display().to_string()
    };
    eprintln!(
        "rendered {} -> {} tile(s):",
        input_name,
        tiled_doc.tile_count()
    );
    for (filename, size) in &files {
        eprintln!("  {} ({} bytes)", filename, size);
    }

    Ok(())
}
