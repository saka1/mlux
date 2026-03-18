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
        ABI, Access, AccessFs, AccessNet, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus,
    };

    pub fn enforce(read_scopes: &[&Path]) -> Result<()> {
        let abi = ABI::V4;

        let read_access = AccessFs::from_read(abi);
        let all_fs_access = AccessFs::from_all(abi);
        let all_net_access = AccessNet::from_all(abi);

        let mut ruleset = Ruleset::default()
            .handle_access(all_fs_access)?
            .handle_access(all_net_access)?
            .create()?;

        for scope in read_scopes {
            ruleset = ruleset.add_rule(PathBeneath::new(PathFd::new(scope)?, read_access))?;
        }

        // No NetPort rules added = all TCP bind/connect denied.

        let status = ruleset.restrict_self()?;

        match status.ruleset {
            RulesetStatus::FullyEnforced => {
                log::info!("sandbox: Landlock fully enforced (V4: FS + network)");
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

    pub fn enforce(_read_scopes: &[&Path]) -> Result<()> {
        log::info!("sandbox: not available on this platform (Linux Landlock only)");
        Ok(())
    }
}

/// Apply filesystem + network sandbox.
///
/// Filesystem: read-only access to `read_base` (expanded to git root).
/// Network: all TCP bind/connect denied (Landlock V4).
/// On V3 kernels, network restriction is silently skipped (graceful degradation).
pub fn enforce_sandbox(read_base: Option<&Path>, font_dirs: &[PathBuf]) -> Result<()> {
    let scope = read_base.map(read_scope);
    let mut read_scopes: Vec<&Path> = Vec::new();

    if let Some(ref s) = scope {
        read_scopes.push(s.as_path());
    }
    for dir in font_dirs {
        read_scopes.push(dir.as_path());
    }

    match imp::enforce(&read_scopes) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!("sandbox: failed to apply Landlock, continuing without sandbox: {e:#}");
            Ok(())
        }
    }
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
