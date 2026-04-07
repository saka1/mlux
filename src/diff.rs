use std::ops::Range;
use std::path::Path;
use std::sync::Once;

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use crate::frame::{VisualLine, byte_offset_to_line};

/// Default line height (pt) used when no next visual line exists for height estimation.
pub const DEFAULT_LINE_HEIGHT_PT: f64 = 14.0;

/// Git diff status for a line or range of lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
}

/// A range of lines with a diff status.
#[derive(Debug, Clone)]
pub struct DiffLineRange {
    /// 0-based line range (exclusive end).
    pub lines: Range<usize>,
    pub status: DiffStatus,
}

/// Get diff line ranges for a file against HEAD.
///
/// Returns empty Vec on any failure (not in a git repo, untracked file,
/// git2 error, etc.).
pub fn diff_against_head(file_path: &Path) -> Vec<DiffLineRange> {
    let ranges = diff_against_head_inner(file_path).unwrap_or_default();
    if !ranges.is_empty() {
        info!("diff: {} hunk(s) for {}", ranges.len(), file_path.display());
        for r in &ranges {
            debug!("  {:?} lines {:?}", r.status, r.lines);
        }
    }
    ranges
}

fn diff_against_head_inner(file_path: &Path) -> Option<Vec<DiffLineRange>> {
    // Disable global/system config search — these paths (e.g. ~/.gitconfig)
    // are outside the Landlock sandbox and would cause "Permission denied".
    // Safe: we only need the repo-local config inside .git/config.
    static GIT2_INIT: Once = Once::new();
    GIT2_INIT.call_once(|| unsafe {
        let _ = git2::opts::set_search_path(git2::ConfigLevel::Global, "");
        let _ = git2::opts::set_search_path(git2::ConfigLevel::System, "");
        let _ = git2::opts::set_search_path(git2::ConfigLevel::XDG, "");
    });

    let parent = file_path.parent()?;
    let repo = match git2::Repository::discover(parent) {
        Ok(r) => r,
        Err(e) => {
            if e.code() == git2::ErrorCode::NotFound {
                debug!("diff: not a git repository");
            } else {
                warn!("diff: Repository::discover failed: {e}");
            }
            return None;
        }
    };
    let workdir = repo.workdir()?;
    let rel_path = file_path.strip_prefix(workdir).ok()?;

    let head = repo.head().ok()?;
    let head_tree = head.peel_to_tree().ok()?;

    let mut opts = git2::DiffOptions::new();
    opts.pathspec(rel_path);
    opts.context_lines(0);

    let diff = repo
        .diff_tree_to_workdir(Some(&head_tree), Some(&mut opts))
        .ok()?;

    let mut ranges = Vec::new();
    diff.foreach(
        &mut |_, _| true,
        None,
        Some(&mut |_, hunk| {
            let new_start = hunk.new_start() as usize; // 1-based
            let new_lines = hunk.new_lines() as usize;
            if new_lines > 0 {
                let status = if hunk.old_lines() == 0 {
                    DiffStatus::Added
                } else {
                    DiffStatus::Modified
                };
                ranges.push(DiffLineRange {
                    lines: (new_start - 1)..(new_start - 1 + new_lines), // 0-based
                    status,
                });
            } else if hunk.old_lines() > 0 {
                // Pure deletion: mark the line just before the gap.
                let mark_line = new_start.saturating_sub(1); // 0-based
                ranges.push(DiffLineRange {
                    lines: mark_line..mark_line + 1,
                    status: DiffStatus::Deleted,
                });
            }
            true
        }),
        None,
    )
    .ok()?;

    Some(ranges)
}

/// Precompute 0-based line number for each visual line (None if unmapped).
fn build_vl_line_index(lines: &[VisualLine], markdown: &str) -> Vec<Option<usize>> {
    lines
        .iter()
        .map(|vl| vl.md_offset.map(|o| byte_offset_to_line(markdown, o) - 1))
        .collect()
}

/// Apply diff status to visual lines based on their Markdown source positions.
pub fn apply_diff_to_visual_lines(
    lines: &mut [VisualLine],
    diff_ranges: &[DiffLineRange],
    markdown: &str,
) {
    let vl_lines = build_vl_line_index(lines, markdown);
    let mut marked = 0usize;
    for (i, vl) in lines.iter_mut().enumerate() {
        if let Some(line_0based) = vl_lines[i] {
            for dr in diff_ranges {
                if dr.lines.contains(&line_0based) {
                    vl.diff_status = Some(dr.status);
                    marked += 1;
                    break;
                }
            }
        }
    }
    info!("diff: {marked}/{} visual lines marked", lines.len());
}

/// Find Y coordinates for deletion markers that couldn't be matched to a visual line.
///
/// When a deletion hunk's `mark_line` points to an empty line (no visual representation),
/// this function computes the Y position of the gap from surrounding visual lines.
/// Returns a list of Y coordinates (in pt) where deletion gap markers should be drawn.
pub fn find_deletion_gaps(
    lines: &[VisualLine],
    diff_ranges: &[DiffLineRange],
    markdown: &str,
) -> Vec<f64> {
    let vl_lines = build_vl_line_index(lines, markdown);
    let mut gaps = Vec::new();

    for dr in diff_ranges {
        if dr.status != DiffStatus::Deleted {
            continue;
        }
        let target = dr.lines.start;

        // Check if this deletion was already matched to a visual line.
        let already_matched = lines.iter().enumerate().any(|(i, vl)| {
            vl.diff_status == Some(DiffStatus::Deleted) && vl_lines[i] == Some(target)
        });
        if already_matched {
            continue;
        }

        // Find the first visual line AFTER the deletion gap.
        let after_idx = vl_lines
            .iter()
            .position(|l| matches!(l, Some(l) if *l > target));

        let gap_y = if let Some(idx) = after_idx {
            lines[idx].y_pt
        } else if let Some(last) = lines.last() {
            last.y_pt + DEFAULT_LINE_HEIGHT_PT
        } else {
            continue;
        };

        gaps.push(gap_y);
    }

    if !gaps.is_empty() {
        info!("diff: {} deletion gap marker(s)", gaps.len());
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vl(y_pt: f64, md_offset: Option<usize>) -> VisualLine {
        VisualLine {
            y_pt,
            y_px: 0,
            md_block_range: md_offset.map(|o| o..o + 1),
            md_offset,
            diff_status: None,
        }
    }

    #[test]
    fn apply_diff_empty_ranges() {
        let mut lines = vec![make_vl(10.0, Some(0)), make_vl(20.0, Some(5))];
        apply_diff_to_visual_lines(&mut lines, &[], "hello\nworld\n");
        assert!(lines.iter().all(|l| l.diff_status.is_none()));
    }

    #[test]
    fn apply_diff_added_line() {
        // "hello\nworld\n" — offset 0 is line 1 (0-based: 0), offset 6 is line 2 (0-based: 1)
        let mut lines = vec![make_vl(10.0, Some(0)), make_vl(20.0, Some(6))];
        let ranges = vec![DiffLineRange {
            lines: 1..2, // line 2 (0-based: 1)
            status: DiffStatus::Added,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, "hello\nworld\n");
        assert_eq!(lines[0].diff_status, None);
        assert_eq!(lines[1].diff_status, Some(DiffStatus::Added));
    }

    #[test]
    fn apply_diff_modified_line() {
        let mut lines = vec![make_vl(10.0, Some(0)), make_vl(20.0, Some(6))];
        let ranges = vec![DiffLineRange {
            lines: 0..1,
            status: DiffStatus::Modified,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, "hello\nworld\n");
        assert_eq!(lines[0].diff_status, Some(DiffStatus::Modified));
        assert_eq!(lines[1].diff_status, None);
    }

    #[test]
    fn apply_diff_unmapped_visual_line() {
        let mut lines = vec![make_vl(10.0, None), make_vl(20.0, Some(0))];
        let ranges = vec![DiffLineRange {
            lines: 0..1,
            status: DiffStatus::Added,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, "hello\n");
        assert_eq!(lines[0].diff_status, None); // unmapped stays None
        assert_eq!(lines[1].diff_status, Some(DiffStatus::Added));
    }

    #[test]
    fn apply_diff_deleted_line() {
        // "hello\nworld\n" — deletion marker at line 1 (0-based: 0)
        let mut lines = vec![make_vl(10.0, Some(0)), make_vl(20.0, Some(6))];
        let ranges = vec![DiffLineRange {
            lines: 0..1,
            status: DiffStatus::Deleted,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, "hello\nworld\n");
        assert_eq!(lines[0].diff_status, Some(DiffStatus::Deleted));
        assert_eq!(lines[1].diff_status, None);
    }

    #[test]
    fn find_deletion_gaps_on_empty_line() {
        // "heading\n\ncontent\n" — deletion marker at line 1 (empty line, 0-based).
        // No visual line maps to line 1, so this becomes a gap marker.
        let md = "heading\n\ncontent\n";
        let mut lines = vec![make_vl(10.0, Some(0)), make_vl(30.0, Some(9))];
        let ranges = vec![DiffLineRange {
            lines: 1..2,
            status: DiffStatus::Deleted,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, md);
        // No visual line should be marked (empty line has no visual line).
        assert_eq!(lines[0].diff_status, None);
        assert_eq!(lines[1].diff_status, None);

        // Gap marker should point to the Y of the first visual line after the gap.
        let gaps = find_deletion_gaps(&lines, &ranges, md);
        assert_eq!(gaps.len(), 1);
        assert!((gaps[0] - 30.0).abs() < f64::EPSILON); // y_pt of "content" line
    }

    #[test]
    fn find_deletion_gaps_exact_match_no_gap() {
        // Deletion marker on a content line (exact match) → no gap needed.
        let md = "hello\nworld\n";
        let mut lines = vec![make_vl(10.0, Some(0)), make_vl(20.0, Some(6))];
        let ranges = vec![DiffLineRange {
            lines: 0..1,
            status: DiffStatus::Deleted,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, md);
        assert_eq!(lines[0].diff_status, Some(DiffStatus::Deleted));

        let gaps = find_deletion_gaps(&lines, &ranges, md);
        assert!(gaps.is_empty());
    }

    #[test]
    fn find_deletion_gaps_at_end() {
        // Deletion after the last content line → gap below the last visual line.
        let md = "hello\n";
        let mut lines = vec![make_vl(10.0, Some(0))];
        let ranges = vec![DiffLineRange {
            lines: 1..2, // after the last line
            status: DiffStatus::Deleted,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, md);
        assert_eq!(lines[0].diff_status, None);

        let gaps = find_deletion_gaps(&lines, &ranges, md);
        assert_eq!(gaps.len(), 1);
        assert!((gaps[0] - (10.0 + DEFAULT_LINE_HEIGHT_PT)).abs() < f64::EPSILON);
    }

    #[test]
    fn find_deletion_gaps_at_line_zero() {
        // Deletion at the very beginning of the file (mark_line=0), where line 0
        // is an empty line with no visual representation.
        let md = "\ncontent\n";
        // Only visual line is "content" at offset 1 (line 1, 0-based).
        let mut lines = vec![make_vl(20.0, Some(1))];
        let ranges = vec![DiffLineRange {
            lines: 0..1,
            status: DiffStatus::Deleted,
        }];
        apply_diff_to_visual_lines(&mut lines, &ranges, md);
        assert_eq!(lines[0].diff_status, None); // line 1 != target line 0

        // Gap should point to the first visual line (the line after the gap).
        let gaps = find_deletion_gaps(&lines, &ranges, md);
        assert_eq!(gaps.len(), 1);
        assert!((gaps[0] - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn diff_against_head_outside_repo() {
        let result = diff_against_head(Path::new("/tmp/nonexistent_file_12345.md"));
        assert!(result.is_empty());
    }

    #[test]
    fn diff_against_head_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // Create and commit a file
        let file_path = dir.path().join("test.md");
        std::fs::write(&file_path, "line1\nline2\nline3\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("test.md")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        // Modify the file (add a line)
        std::fs::write(&file_path, "line1\nline2\nline3\nnew line\n").unwrap();

        let ranges = diff_against_head(&file_path);
        assert!(!ranges.is_empty());
        // The added line (line 4, 0-based: 3) should be Added
        let added = ranges.iter().find(|r| r.lines.contains(&3)).unwrap();
        assert_eq!(added.status, DiffStatus::Added);
    }

    #[test]
    fn diff_against_head_modified_line() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        let file_path = dir.path().join("test.md");
        std::fs::write(&file_path, "line1\nline2\nline3\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("test.md")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        // Modify line 2
        std::fs::write(&file_path, "line1\nchanged\nline3\n").unwrap();

        let ranges = diff_against_head(&file_path);
        assert!(!ranges.is_empty());
        let modified = ranges.iter().find(|r| r.lines.contains(&1)).unwrap();
        assert_eq!(modified.status, DiffStatus::Modified);
    }

    #[test]
    fn diff_against_head_deleted_line() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        let file_path = dir.path().join("test.md");
        std::fs::write(&file_path, "line1\nline2\nline3\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("test.md")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        // Delete line2
        std::fs::write(&file_path, "line1\nline3\n").unwrap();

        let ranges = diff_against_head(&file_path);
        assert!(!ranges.is_empty());
        let deleted = ranges
            .iter()
            .find(|r| r.status == DiffStatus::Deleted)
            .unwrap();
        // Deletion between line 1 and line 2 in new file → marks line 1 (0-based)
        assert!(deleted.lines.contains(&0) || deleted.lines.contains(&1));
    }
}
