use std::path::{Path, PathBuf};

use anyhow::Result;

/// Find git repository root by walking up from `start` looking for `.git`.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Determine the read scope for sandboxing.
/// Uses git root if the path is inside a git repo, otherwise the path itself.
fn read_scope(base: &Path) -> PathBuf {
    find_git_root(base).unwrap_or_else(|| base.to_path_buf())
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::Path;

    use anyhow::Result;
    use landlock::{
        ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus,
    };

    pub fn enforce(read_scopes: &[&Path], write_scope: Option<&Path>) -> Result<()> {
        let abi = ABI::V3;

        let read_access = AccessFs::from_read(abi);
        let write_access = AccessFs::from_write(abi);
        let all_access = AccessFs::from_all(abi);

        let mut ruleset = Ruleset::default().handle_access(all_access)?.create()?;

        for scope in read_scopes {
            ruleset = ruleset.add_rule(PathBeneath::new(PathFd::new(scope)?, read_access))?;
        }

        if let Some(scope) = write_scope {
            ruleset = ruleset.add_rule(PathBeneath::new(PathFd::new(scope)?, write_access))?;
        }

        let status = ruleset.restrict_self()?;

        match status.ruleset {
            RulesetStatus::FullyEnforced => {
                log::info!("sandbox: Landlock fully enforced");
            }
            RulesetStatus::PartiallyEnforced => {
                log::warn!("sandbox: Landlock partially enforced (some rules unsupported)");
            }
            RulesetStatus::NotEnforced => {
                log::warn!("sandbox: Landlock not enforced (kernel may not support it)");
            }
        }

        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::path::Path;

    use anyhow::Result;

    pub fn enforce(_read_scopes: &[&Path], _write_scope: Option<&Path>) -> Result<()> {
        log::info!("sandbox: not available on this platform (Linux Landlock only)");
        Ok(())
    }
}

/// System paths needed for DNS resolution and TLS certificate verification.
const NETWORK_SYSTEM_PATHS: &[&str] = &[
    "/etc",     // resolv.conf, hosts, nsswitch.conf, ssl/certs, ca-certificates
    "/usr/lib", // NSS shared libraries (libnss_dns.so, etc.)
    "/run",     // systemd-resolved stub resolver config
];

/// Apply filesystem sandbox with optional network system path access.
///
/// When `allow_network` is true, adds read access to system paths required
/// for DNS resolution and TLS (e.g. `/etc/resolv.conf`, `/etc/ssl/certs`).
pub fn enforce_sandbox(read_base: Option<&Path>, allow_network: bool) -> Result<()> {
    let scope = read_base.map(read_scope);
    let mut read_scopes: Vec<&Path> = Vec::new();

    if let Some(ref s) = scope {
        read_scopes.push(s.as_path());
    }

    let network_paths: Vec<PathBuf>;
    let resolv_target_dir: Option<PathBuf>;
    if allow_network {
        network_paths = NETWORK_SYSTEM_PATHS
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .collect();
        for p in &network_paths {
            log::info!("sandbox: adding network system path: {}", p.display());
            read_scopes.push(p.as_path());
        }

        // Landlock checks access against resolved (real) paths, not symlink paths.
        // /etc/resolv.conf is often a symlink (e.g. → /mnt/wsl/resolv.conf on WSL2,
        // → /run/systemd/resolve/stub-resolv.conf on systemd-resolved).
        // Add the symlink target's parent directory if not already covered.
        resolv_target_dir = std::fs::canonicalize("/etc/resolv.conf")
            .ok()
            .and_then(|real| real.parent().map(|p| p.to_path_buf()))
            .filter(|parent| !read_scopes.iter().any(|s| parent.starts_with(s)));
        if let Some(ref dir) = resolv_target_dir {
            log::info!(
                "sandbox: adding resolv.conf symlink target dir: {}",
                dir.display()
            );
            read_scopes.push(dir.as_path());
        }
    }

    match imp::enforce(&read_scopes, None) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!("sandbox: failed to apply Landlock, continuing without sandbox: {e:#}");
            Ok(())
        }
    }
}

/// Apply read-only filesystem sandbox (no write access, no network paths).
///
/// Used by fork child processes that only need to compile/render.
pub fn enforce_read_only_sandbox(read_base: Option<&Path>) -> Result<()> {
    enforce_sandbox(read_base, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_git_root_from_subdir() {
        // This test runs within the mlux repo, so we should find a git root
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src_dir = manifest_dir.join("src");
        let root = find_git_root(&src_dir);
        assert!(root.is_some());
        assert!(root.unwrap().join(".git").exists());
    }

    #[test]
    fn find_git_root_returns_none_for_root() {
        let root = find_git_root(Path::new("/"));
        assert!(root.is_none());
    }

    #[test]
    fn read_scope_uses_git_root() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let scope = read_scope(manifest_dir);
        assert!(scope.join(".git").exists());
    }
}
