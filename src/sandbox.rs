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

    pub fn enforce(read_scope: Option<&Path>, write_scope: Option<&Path>) -> Result<()> {
        let abi = ABI::V1;

        let read_access = AccessFs::from_read(abi);
        let write_access = AccessFs::from_write(abi);
        let all_access = AccessFs::from_all(abi);

        let mut ruleset = Ruleset::default().handle_access(all_access)?.create()?;

        if let Some(scope) = read_scope {
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

    pub fn enforce(_read_scope: Option<&Path>, _write_scope: Option<&Path>) -> Result<()> {
        log::info!("sandbox: not available on this platform (Linux Landlock only)");
        Ok(())
    }
}

/// Apply filesystem sandbox for render mode.
///
/// - `read_base`: input file's parent directory (canonicalized). None for stdin.
/// - `output_dir`: output directory (canonicalized).
/// - `no_sandbox`: skip if true.
pub fn enforce_fs_sandbox(
    read_base: Option<&Path>,
    output_dir: &Path,
    no_sandbox: bool,
) -> Result<()> {
    if no_sandbox {
        log::info!("sandbox: disabled by --no-sandbox flag");
        return Ok(());
    }

    let scope = read_base.map(read_scope);

    match imp::enforce(scope.as_deref(), Some(output_dir)) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!("sandbox: failed to apply Landlock, continuing without sandbox: {e:#}");
            Ok(())
        }
    }
}

/// Apply read-only filesystem sandbox (no write access).
///
/// Used by fork child processes that only need to compile/render.
pub fn enforce_read_only_sandbox(read_base: Option<&Path>) -> Result<()> {
    let scope = read_base.map(read_scope);

    match imp::enforce(scope.as_deref(), None) {
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
