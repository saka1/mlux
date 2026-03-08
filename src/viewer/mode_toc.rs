//! TOC overlay mode: list document headings and jump to them.

use crossterm::{
    QueueableCommand, cursor,
    style::{self, Stylize},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write, stdout};

use super::input::TocAction;
use super::state::{ExitReason, Layout, visual_line_offset};
use super::{Effect, ViewerMode};
use crate::tile::VisualLine;

/// A single heading entry in the TOC.
pub(super) struct TocEntry {
    pub level: u8,
    pub text: String,
    pub md_line: usize,
    pub visual_line_idx: usize,
}

/// Mutable state for TOC overlay mode.
pub(super) struct TocState {
    pub entries: Vec<TocEntry>,
    pub selected: usize,
    pub scroll_offset: usize,
}

impl TocState {
    pub(super) fn new(entries: Vec<TocEntry>) -> Self {
        Self {
            entries,
            selected: 0,
            scroll_offset: 0,
        }
    }
}

/// Collect headings from markdown source and map them to visual lines.
///
/// Parses ATX headings (`# heading`) by scanning lines directly.
/// Lines inside fenced code blocks are ignored.
pub(super) fn collect_headings(markdown: &str, visual_lines: &[VisualLine]) -> Vec<TocEntry> {
    let mut entries = Vec::new();
    let mut in_code_block = false;

    for (line_idx, line) in markdown.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        // Match ATX headings: 1-6 '#' followed by space
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if hashes == 0 || hashes > 6 {
            continue;
        }
        let rest = &trimmed[hashes..];
        if !rest.starts_with(' ') {
            continue;
        }
        let text = rest.trim().to_string();
        if text.is_empty() {
            continue;
        }

        let md_line = line_idx + 1; // 1-based
        if let Some(vl_idx) = find_visual_line(visual_lines, md_line) {
            entries.push(TocEntry {
                level: hashes as u8,
                text,
                md_line,
                visual_line_idx: vl_idx,
            });
        }
    }
    entries
}

/// Find the visual line index that contains the given 1-based markdown line.
fn find_visual_line(visual_lines: &[VisualLine], md_line: usize) -> Option<usize> {
    visual_lines.iter().position(|vl| {
        vl.md_line_range
            .is_some_and(|(s, e)| md_line >= s && md_line <= e)
    })
}

/// Draw the TOC overlay screen.
pub(super) fn draw_toc_screen(layout: &Layout, state: &TocState) -> io::Result<()> {
    let mut out = stdout();
    out.queue(Clear(ClearType::All))?;

    let total_cols = (layout.sidebar_cols + layout.image_cols) as usize;

    // Row 0: header
    out.queue(cursor::MoveTo(0, 0))?;
    let header = " Table of Contents:";
    write!(out, "{}", header.white().bold())?;

    // Result list: rows 1 .. status_row-1
    let list_start_row: u16 = 1;
    let list_end_row = layout.status_row;
    let visible_count = (list_end_row - list_start_row) as usize;

    for i in 0..visible_count {
        let entry_idx = state.scroll_offset + i;
        let row = list_start_row + i as u16;
        out.queue(cursor::MoveTo(0, row))?;

        if entry_idx >= state.entries.len() {
            write!(out, "{:width$}", "", width = total_cols)?;
            continue;
        }

        let e = &state.entries[entry_idx];
        let is_selected = entry_idx == state.selected;

        let indent = (e.level as usize - 1) * 2;
        let marker = if is_selected { " > " } else { "   " };
        let line_label = format!("L{:<4}", e.md_line);
        let content = format!(
            "{marker}{line_label} {:indent$}\u{2022} {}",
            "",
            e.text,
            indent = indent
        );

        let display: String = content.chars().take(total_cols).collect();
        let pad = total_cols.saturating_sub(display.len());

        if is_selected {
            write!(
                out,
                "{}",
                format!("{display}{:pad$}", "").on_dark_blue().white()
            )?;
        } else {
            write!(out, "{display}{:pad$}", "")?;
        }
    }

    // Status line
    out.queue(cursor::MoveTo(0, layout.status_row))?;
    let status = format!(
        " {} heading{} | Enter:jump  j/k:select  Esc:cancel",
        state.entries.len(),
        if state.entries.len() == 1 { "" } else { "s" }
    );
    let padded = format!("{:<width$}", status, width = total_cols);
    write!(out, "{}", padded.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;

    out.flush()
}

/// Handle a TOC action, returning effects.
pub(super) fn handle(
    action: TocAction,
    state: &mut TocState,
    visual_lines: &[VisualLine],
    visible_count: usize,
    max_scroll: u32,
) -> Vec<Effect> {
    match action {
        TocAction::Quit => vec![Effect::Exit(ExitReason::Quit)],
        TocAction::SelectNext => {
            if !state.entries.is_empty() {
                state.selected = (state.selected + 1).min(state.entries.len() - 1);
                if state.selected >= state.scroll_offset + visible_count {
                    state.scroll_offset = state.selected - visible_count + 1;
                }
            }
            vec![Effect::RedrawToc]
        }
        TocAction::SelectPrev => {
            if !state.entries.is_empty() {
                state.selected = state.selected.saturating_sub(1);
                if state.selected < state.scroll_offset {
                    state.scroll_offset = state.selected;
                }
            }
            vec![Effect::RedrawToc]
        }
        TocAction::Confirm => {
            if state.entries.is_empty() {
                return vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty];
            }
            let vl_idx = state.entries[state.selected].visual_line_idx;
            let line_num = (vl_idx + 1) as u32; // 1-based
            let y = visual_line_offset(visual_lines, max_scroll, line_num);
            let heading = state.entries[state.selected].text.clone();
            vec![
                Effect::ScrollTo(y),
                Effect::Flash(format!("Jumped to: {heading}")),
                Effect::SetMode(ViewerMode::Normal),
            ]
        }
        TocAction::Cancel => vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vl(md_line_range: Option<(usize, usize)>) -> VisualLine {
        VisualLine {
            y_pt: 0.0,
            y_px: 0,
            md_line_range,
            md_line_exact: None,
        }
    }

    #[test]
    fn collect_headings_basic() {
        let md = "# Title\n\nSome text\n\n## Section 1\n\n### Subsection\n";
        let vls = vec![
            make_vl(Some((1, 1))),
            make_vl(Some((3, 3))),
            make_vl(Some((5, 5))),
            make_vl(Some((7, 7))),
        ];
        let entries = collect_headings(md, &vls);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].level, 1);
        assert_eq!(entries[0].text, "Title");
        assert_eq!(entries[1].level, 2);
        assert_eq!(entries[1].text, "Section 1");
        assert_eq!(entries[2].level, 3);
        assert_eq!(entries[2].text, "Subsection");
    }

    #[test]
    fn collect_headings_empty_document() {
        let md = "No headings here.\nJust text.\n";
        let vls = vec![make_vl(Some((1, 1))), make_vl(Some((2, 2)))];
        let entries = collect_headings(md, &vls);
        assert!(entries.is_empty());
    }

    #[test]
    fn collect_headings_ignores_code_blocks() {
        let md = "# Real heading\n\n```\n# Not a heading\n```\n\n## Also real\n";
        let vls = vec![
            make_vl(Some((1, 1))),
            make_vl(Some((3, 5))),
            make_vl(Some((7, 7))),
        ];
        let entries = collect_headings(md, &vls);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "Real heading");
        assert_eq!(entries[1].text, "Also real");
    }

    #[test]
    fn collect_headings_all_levels() {
        let md = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n####### Not a heading\n";
        let vls: Vec<_> = (1..=7).map(|i| make_vl(Some((i, i)))).collect();
        let entries = collect_headings(md, &vls);
        assert_eq!(entries.len(), 6);
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.level, (i + 1) as u8);
        }
    }

    #[test]
    fn handle_select_next() {
        let entries = vec![
            TocEntry {
                level: 1,
                text: "A".into(),
                md_line: 1,
                visual_line_idx: 0,
            },
            TocEntry {
                level: 2,
                text: "B".into(),
                md_line: 3,
                visual_line_idx: 2,
            },
        ];
        let mut state = TocState::new(entries);
        assert_eq!(state.selected, 0);
        let vls = vec![make_vl(Some((1, 1)))];
        let _ = handle(TocAction::SelectNext, &mut state, &vls, 20, 1000);
        assert_eq!(state.selected, 1);
        // Clamp at end
        let _ = handle(TocAction::SelectNext, &mut state, &vls, 20, 1000);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn handle_select_prev() {
        let entries = vec![
            TocEntry {
                level: 1,
                text: "A".into(),
                md_line: 1,
                visual_line_idx: 0,
            },
            TocEntry {
                level: 2,
                text: "B".into(),
                md_line: 3,
                visual_line_idx: 2,
            },
        ];
        let mut state = TocState::new(entries);
        state.selected = 1;
        let vls = vec![make_vl(Some((1, 1)))];
        let _ = handle(TocAction::SelectPrev, &mut state, &vls, 20, 1000);
        assert_eq!(state.selected, 0);
        // Clamp at 0
        let _ = handle(TocAction::SelectPrev, &mut state, &vls, 20, 1000);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn handle_confirm_scrolls_and_returns_normal() {
        let entries = vec![
            TocEntry {
                level: 1,
                text: "Title".into(),
                md_line: 1,
                visual_line_idx: 0,
            },
            TocEntry {
                level: 2,
                text: "Section".into(),
                md_line: 5,
                visual_line_idx: 3,
            },
        ];
        let mut state = TocState::new(entries);
        state.selected = 1;
        let vls = vec![
            make_vl(Some((1, 1))),
            make_vl(Some((2, 2))),
            make_vl(Some((3, 4))),
            make_vl(Some((5, 5))),
        ];
        let effects = handle(TocAction::Confirm, &mut state, &vls, 20, 1000);
        assert!(effects.iter().any(|e| matches!(e, Effect::ScrollTo(_))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
    }

    #[test]
    fn handle_cancel_returns_normal() {
        let entries = vec![TocEntry {
            level: 1,
            text: "A".into(),
            md_line: 1,
            visual_line_idx: 0,
        }];
        let mut state = TocState::new(entries);
        let vls = vec![make_vl(Some((1, 1)))];
        let effects = handle(TocAction::Cancel, &mut state, &vls, 20, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkDirty)));
    }
}
