use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use tmark::convert::markdown_to_typst;
use tmark::render::render_to_png;
use tmark::world::TmarkWorld;

#[derive(Parser)]
#[command(name = "tmark", about = "Render Markdown to PNG via Typst")]
struct Cli {
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

    /// Theme name (loaded from themes/{name}.typ)
    #[arg(long, default_value = "catppuccin")]
    theme: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Read input markdown
    let markdown = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read {}", cli.input.display()))?;

    // Read theme file
    let theme_path = PathBuf::from(format!("themes/{}.typ", cli.theme));
    let theme_text = fs::read_to_string(&theme_path)
        .with_context(|| format!("failed to read theme {}", theme_path.display()))?;

    // Convert markdown to typst
    let content_text = markdown_to_typst(&markdown);

    // Create world and render
    let world = TmarkWorld::new(&theme_text, &content_text, cli.width);
    let png_data = render_to_png(&world, cli.ppi)?;

    // Write output
    if let Some(parent) = cli.output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&cli.output, &png_data)
        .with_context(|| format!("failed to write {}", cli.output.display()))?;

    eprintln!(
        "rendered {} -> {} ({} bytes)",
        cli.input.display(),
        cli.output.display(),
        png_data.len()
    );

    Ok(())
}
