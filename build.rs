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

    // Embed fonts from fonts/ directory (only when embed-noto-fonts feature is enabled)
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    let mut font_files: Vec<String> = Vec::new();
    if env::var("CARGO_FEATURE_EMBED_NOTO_FONTS").is_ok() {
        let font_dir = Path::new("fonts");
        if font_dir.is_dir() {
            for entry in fs::read_dir(font_dir).unwrap().flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension()
                    && (ext == "ttf" || ext == "otf")
                {
                    // Use absolute path for include_bytes! (relative path from OUT_DIR is unstable)
                    let abs = format!("{}/{}", manifest_dir, path.display());
                    font_files.push(abs);
                }
            }
        }
        font_files.sort(); // Deterministic build output
    }

    let dest = Path::new(&out_dir).join("embedded_fonts.rs");
    let mut f = fs::File::create(&dest).unwrap();

    if font_files.is_empty() {
        writeln!(
            f,
            "fn embedded_font_data() -> &'static [&'static [u8]] {{ &[] }}"
        )
        .unwrap();
    } else {
        writeln!(f, "fn embedded_font_data() -> &'static [&'static [u8]] {{").unwrap();
        writeln!(f, "    &[").unwrap();
        for path in &font_files {
            writeln!(f, "        include_bytes!(\"{}\"),", path).unwrap();
        }
        writeln!(f, "    ]").unwrap();
        writeln!(f, "}}").unwrap();
    }

    println!("cargo:rerun-if-changed=fonts/");
}
