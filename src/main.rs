use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::info;

use mlux::config;
use mlux::input::{self, InputSource};
use mlux::pipeline::dump_document;
use mlux::pipeline::{self, BuildParams, FontCache, MluxWorld};

/// Default sidebar width in typst points for headless (non-terminal) rendering.
const DEFAULT_SIDEBAR_WIDTH_PT: f64 = 40.0;

fn long_version() -> &'static str {
    let base = env!("CARGO_PKG_VERSION");
    let hash = option_env!("MLUX_BUILD_GIT_HASH").unwrap_or("");
    let profile = option_env!("MLUX_BUILD_PROFILE").unwrap_or("unknown");
    let describe = option_env!("MLUX_BUILD_GIT_DESCRIBE").unwrap_or("");

    // git describe --tags --always output patterns:
    //   ""                     → no git (tarball, crates.io): use Cargo version
    //   starts with 'v'       → tag present: use as-is (e.g. "v0.4.1" or "v0.4.1-3-ge0e4555")
    //   otherwise             → no tags (shallow clone etc): "{base}-dev+{hash}"
    let version = if describe.is_empty() {
        base.to_string()
    } else if describe.starts_with('v') {
        describe.to_string()
    } else {
        format!("{base}-dev+{describe}")
    };

    if hash.is_empty() {
        format!("{version} ({profile})").leak()
    } else {
        format!("{version} (rev {hash}, {profile})").leak()
    }
}

#[derive(Parser)]
#[command(name = "mlux", version = long_version(), about = "Markdown viewer and renderer powered by Typst")]
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

    /// Disable Landlock filesystem sandbox (Linux only; fork is always used)
    #[arg(long, global = true)]
    no_sandbox: bool,

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
        }) => cmd_render(input, &config, output, dump, cli.no_sandbox),
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
            mlux::viewer::run(
                input_source,
                config,
                &cli_overrides,
                !cli.no_watch,
                cli.no_sandbox,
            )
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
    config: &config::Config,
    output: PathBuf,
    dump: bool,
    no_sandbox: bool,
) -> Result<()> {
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

    // Load images
    let base_dir = if is_stdin { None } else { input.parent() };
    let image_paths = pipeline::extract_image_paths(&markdown);
    let (image_files, image_errors) = mlux::image::load_images(&image_paths, base_dir);
    for err in &image_errors {
        eprintln!("warning: {err}");
    }
    let loaded_set = image_files.key_set();

    // Convert markdown to typst
    let (content_text, source_map) = pipeline::markdown_to_typst(&markdown, Some(&loaded_set));

    if dump {
        let font_cache = FontCache::new();
        // Dump mode: build world directly for source inspection
        let world = MluxWorld::new(
            theme_text,
            mlux::theme::data_files(theme),
            &content_text,
            width,
            &font_cache,
            image_files.clone(),
        );
        let source_text = world.main_source().text();
        eprintln!(
            "=== Generated main.typ ({} lines) ===",
            source_text.lines().count()
        );
        for (i, line) in source_text.lines().enumerate() {
            eprintln!("{:>4} | {}", i + 1, line);
        }
        eprintln!();

        match pipeline::compile_document(&world) {
            Ok(doc) => dump_document(&doc),
            Err(e) => eprintln!("{e:#}"),
        }
        return Ok(());
    }

    let read_base = if is_stdin {
        None
    } else {
        Some(
            input
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .canonicalize()
                .context("failed to canonicalize input directory")?,
        )
    };
    let output_parent = output.parent().unwrap_or_else(|| std::path::Path::new("."));
    fs::create_dir_all(output_parent).ok();

    let data_files = mlux::theme::data_files(theme);

    let font_cache = FontCache::new();
    let params = BuildParams {
        theme_text,
        data_files,
        content_text: &content_text,
        md_source: &markdown,
        source_map: &source_map,
        width_pt: width,
        sidebar_width_pt: DEFAULT_SIDEBAR_WIDTH_PT,
        tile_height_pt: tile_height,
        ppi,
        fonts: &font_cache,
        image_files,
    };

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

    cmd_render_fork(
        &params,
        read_base.as_deref(),
        output_parent,
        &stem,
        &ext,
        is_stdin,
        &input,
        pipeline_start,
        no_sandbox,
    )
}

/// Render via fork+IPC: child compiles/renders in a forked process.
#[allow(clippy::too_many_arguments)]
fn cmd_render_fork(
    params: &BuildParams<'_>,
    read_base: Option<&Path>,
    output_parent: &Path,
    stem: &str,
    ext: &str,
    is_stdin: bool,
    input: &Path,
    pipeline_start: Instant,
    no_sandbox: bool,
) -> Result<()> {
    use mlux::fork_render::{Request, Response, spawn_renderer};

    let (meta, mut tx, mut rx, mut _child) = spawn_renderer(params, read_base, no_sandbox)?;

    let mut files = Vec::new();
    for i in 0..meta.tile_count {
        tx.send(&Request::RenderTile(i))?;
        match rx.recv()? {
            Response::Tile(pngs) => {
                let filename = format!("{}-{:03}.{}", stem, i, ext);
                let path = output_parent.join(&filename);
                fs::write(&path, &pngs.content)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                files.push((filename, pngs.content.len()));
            }
            Response::Error(e) => anyhow::bail!("render tile {i}: {e}"),
            Response::Meta(_) => anyhow::bail!("unexpected Meta response"),
        }
    }
    tx.send(&Request::Shutdown)?;

    info!(
        "cmd_render: total pipeline completed in {:.1}ms",
        pipeline_start.elapsed().as_secs_f64() * 1000.0
    );

    let input_name = if is_stdin {
        "<stdin>".to_string()
    } else {
        input.display().to_string()
    };
    eprintln!("rendered {} -> {} tile(s):", input_name, meta.tile_count);
    for (filename, size) in &files {
        eprintln!("  {} ({} bytes)", filename, size);
    }

    Ok(())
}
