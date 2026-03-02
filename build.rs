use std::{env, fs, io::Write, path::Path, process::Command};

fn main() {
    // Re-run when HEAD changes (new commit, branch switch, etc.)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    // Git commit hash (short, 10 chars)
    let hash = Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    // git describe (tag + distance + hash, or hash only)
    let describe = Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    // Cargo build profile (debug / release)
    let profile = env::var("PROFILE").unwrap_or_default();

    println!("cargo:rustc-env=MLUX_BUILD_GIT_HASH={hash}");
    println!("cargo:rustc-env=MLUX_BUILD_GIT_DESCRIBE={describe}");
    println!("cargo:rustc-env=MLUX_BUILD_PROFILE={profile}");

    // Compress and embed fonts from fonts/ directory (only with embed-noto-fonts feature).
    // Each font is zstd-compressed at build time and decompressed at runtime in world.rs.
    let out_dir = env::var("OUT_DIR").unwrap();
    let compressed_paths = compress_fonts(&out_dir);
    generate_font_data_source(&out_dir, &compressed_paths);
    println!("cargo:rerun-if-changed=fonts/");
}

/// Collect font files from fonts/, compress each with zstd, and write to OUT_DIR.
/// Returns the list of compressed file paths (empty if the feature is disabled or no fonts found).
fn compress_fonts(out_dir: &str) -> Vec<String> {
    if env::var("CARGO_FEATURE_EMBED_NOTO_FONTS").is_err() {
        return Vec::new();
    }

    let font_dir = Path::new("fonts");
    if !font_dir.is_dir() {
        return Vec::new();
    }

    let mut font_paths: Vec<_> = fs::read_dir(font_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("ttf" | "otf")))
        .collect();
    font_paths.sort(); // Deterministic build output

    let mut compressed_paths = Vec::new();
    for path in &font_paths {
        let raw =
            fs::read(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let compressed = zstd::encode_all(&raw[..], 9)
            .unwrap_or_else(|e| panic!("failed to compress {}: {e}", path.display()));
        let dest = Path::new(out_dir).join(format!(
            "{}.zst",
            path.file_name().unwrap().to_str().unwrap()
        ));
        fs::write(&dest, &compressed).unwrap();
        compressed_paths.push(dest.display().to_string());
    }
    compressed_paths
}

/// Generate embedded_fonts.rs containing include_bytes! references to the compressed fonts.
fn generate_font_data_source(out_dir: &str, compressed_paths: &[String]) {
    let dest = Path::new(out_dir).join("embedded_fonts.rs");
    let mut f = fs::File::create(&dest).unwrap();

    writeln!(f, "fn embedded_font_data() -> &'static [&'static [u8]] {{").unwrap();
    if compressed_paths.is_empty() {
        writeln!(f, "    &[]").unwrap();
    } else {
        writeln!(f, "    &[").unwrap();
        for path in compressed_paths {
            writeln!(f, "        include_bytes!(\"{path}\"),").unwrap();
        }
        writeln!(f, "    ]").unwrap();
    }
    writeln!(f, "}}").unwrap();
}
