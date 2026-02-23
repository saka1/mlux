use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use tmark::convert::markdown_to_typst;
use tmark::render::{dump_document, render_to_png};
use tmark::world::TmarkWorld;

#[derive(Parser)]
#[command(name = "tmark", about = "Markdown viewer and renderer powered by Typst")]
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

        /// Dump frame tree to stderr
        #[arg(long)]
        dump: bool,
    },
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Render { input, output, width, ppi, dump }) => {
            cmd_render(input, cli.theme, output, width, ppi, dump)
        }
        None => {
            let input = cli.input
                .ok_or_else(|| anyhow::anyhow!("input file required"))?;
            tmark::viewer::run(input, cli.theme)
        }
    }
}

fn cmd_render(
    input: PathBuf,
    theme: String,
    output: PathBuf,
    width: f64,
    ppi: f32,
    dump: bool,
) -> Result<()> {
    // Read input markdown
    let markdown = fs::read_to_string(&input)
        .with_context(|| format!("failed to read {}", input.display()))?;

    // Read theme file
    let theme_path = PathBuf::from(format!("themes/{}.typ", theme));
    let theme_text = fs::read_to_string(&theme_path)
        .with_context(|| format!("failed to read theme {}", theme_path.display()))?;

    // Convert markdown to typst
    let content_text = markdown_to_typst(&markdown);

    // Create world and render
    let world = TmarkWorld::new(&theme_text, &content_text, width);
    if dump {
        let warned = typst::compile::<typst::layout::PagedDocument>(&world);
        for w in &warned.warnings {
            eprintln!("typst warning: {}", w.message);
        }
        match warned.output {
            Ok(doc) => dump_document(&doc),
            Err(errors) => {
                for e in &errors {
                    eprintln!("typst error: {}", e.message);
                }
            }
        }
        return Ok(());
    }
    let png_data = render_to_png(&world, ppi)?;

    // Write output
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&output, &png_data)
        .with_context(|| format!("failed to write {}", output.display()))?;

    eprintln!(
        "rendered {} -> {} ({} bytes)",
        input.display(),
        output.display(),
        png_data.len()
    );

    Ok(())
}
