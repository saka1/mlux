use std::process::Command;

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

    // Cargo build profile (debug / release)
    let profile = std::env::var("PROFILE").unwrap_or_default();

    println!("cargo:rustc-env=MLUX_BUILD_GIT_HASH={hash}");
    println!("cargo:rustc-env=MLUX_BUILD_PROFILE={profile}");
}
