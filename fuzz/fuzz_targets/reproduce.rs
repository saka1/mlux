use mlux::convert::markdown_to_typst;
use mlux::render::compile_document;
use mlux::world::MluxWorld;

static THEME: &str = include_str!("../../themes/catppuccin.typ");

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: reproduce <artifact-file>");
        std::process::exit(1);
    });

    let data = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        std::process::exit(1);
    });

    let markdown = match std::str::from_utf8(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Input is not valid UTF-8: {e}");
            std::process::exit(1);
        }
    };

    let typst_content = markdown_to_typst(markdown);
    let world = MluxWorld::new(THEME, &typst_content, 660.0);

    println!("=== Input Markdown ({} bytes) ===", markdown.len());
    println!("{markdown}");
    println!();
    println!("=== Generated Typst ({} bytes) ===", typst_content.len());
    println!("{typst_content}");
    println!();

    match compile_document(&world) {
        Ok(_) => println!("=== Compile OK ==="),
        Err(e) => {
            println!("=== Compile Error ===");
            println!("{e}");
            std::process::exit(1);
        }
    }
}
