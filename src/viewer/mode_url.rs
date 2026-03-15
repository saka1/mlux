//! URL picker mode: list and open URLs from the document.

use crossterm::{
    QueueableCommand, cursor,
    style::{self, Stylize},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write, stdout};

use super::input::UrlAction;
use super::layout::Layout;
use super::{Effect, ViewerMode};
use crate::tile::{UrlEntry, VisualLine, extract_urls_from_lines};

/// A single entry in the URL picker list.
pub(super) struct UrlPickerEntry {
    pub url: String,
    pub text: String,
    /// 1-based visual line number (for display).
    pub visual_line: usize,
}

/// Mutable state for URL picker mode.
pub(super) struct UrlPickerState {
    pub entries: Vec<UrlPickerEntry>,
    pub selected: usize,
    pub scroll_offset: usize,
}

impl UrlPickerState {
    pub(super) fn new(entries: Vec<UrlPickerEntry>) -> Self {
        Self {
            entries,
            selected: 0,
            scroll_offset: 0,
        }
    }
}

/// Collect all URL entries from the entire document.
///
/// Iterates over all visual lines, extracts URLs from each line's Markdown
/// source range, and tags each entry with its 1-based visual line number.
pub(super) fn collect_all_url_entries(
    md_source: &str,
    visual_lines: &[VisualLine],
) -> Vec<UrlPickerEntry> {
    let mut entries = Vec::new();
    // Track which md_block_ranges we've already processed to avoid duplicates.
    let mut seen_ranges: Vec<(usize, usize)> = Vec::new();

    for (vl_idx, vl) in visual_lines.iter().enumerate() {
        let Some(ref r) = vl.md_block_range else {
            continue;
        };
        // Skip if we've already extracted URLs from this exact range.
        if seen_ranges.contains(&(r.start, r.end)) {
            continue;
        }
        seen_ranges.push((r.start, r.end));

        let start = crate::tile::byte_offset_to_line(md_source, r.start);
        let end = crate::tile::byte_offset_to_line(md_source, r.end.saturating_sub(1).max(r.start));
        let url_entries = extract_urls_from_lines(md_source, start, end);
        let line_num = vl_idx + 1; // 1-based
        for UrlEntry { url, text } in url_entries {
            entries.push(UrlPickerEntry {
                url,
                text,
                visual_line: line_num,
            });
        }
    }
    entries
}

/// Draw the URL picker screen (replaces tile images).
///
/// Layout:
///   Row 0: " URLs:" header
///   Row 1..N: URL list (scrolled, with selection highlight)
///   Last row: status line with count and key hints
pub(super) fn draw_url_screen(layout: &Layout, state: &UrlPickerState) -> io::Result<()> {
    let mut out = stdout();
    out.queue(Clear(ClearType::All))?;

    let total_cols = (layout.sidebar_cols + layout.image_cols) as usize;

    // Row 0: header
    out.queue(cursor::MoveTo(0, 0))?;
    let header = " URLs:";
    write!(out, "{}", header.white().bold())?;

    // Result list: rows 1 .. status_row-1
    let list_start_row: u16 = 1;
    let list_end_row = layout.status_row; // exclusive
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

        // Format: " > L{line}  [{text}] {url}" or "   L{line}  [{text}] {url}"
        let marker = if is_selected { " > " } else { "   " };
        let line_label = format!("L{:<4}", e.visual_line);

        let content = if e.text.is_empty() {
            format!("{marker}{line_label} {}", e.url)
        } else {
            format!("{marker}{line_label} [{}] {}", e.text, e.url)
        };

        // Truncate to terminal width
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
        " {} URL{} | Enter:open  j/k:select  Esc:cancel",
        state.entries.len(),
        if state.entries.len() == 1 { "" } else { "s" }
    );
    let padded = format!("{:<width$}", status, width = total_cols);
    write!(out, "{}", padded.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;

    out.flush()
}

/// Handle a URL picker action, returning effects.
pub(super) fn handle(
    action: UrlAction,
    state: &mut UrlPickerState,
    visible_count: usize,
) -> Vec<Effect> {
    match action {
        UrlAction::SelectNext => {
            if !state.entries.is_empty() {
                state.selected = (state.selected + 1).min(state.entries.len() - 1);
                if state.selected >= state.scroll_offset + visible_count {
                    state.scroll_offset = state.selected - visible_count + 1;
                }
            }
            vec![Effect::RedrawUrlPicker]
        }
        UrlAction::SelectPrev => {
            if !state.entries.is_empty() {
                state.selected = state.selected.saturating_sub(1);
                if state.selected < state.scroll_offset {
                    state.scroll_offset = state.selected;
                }
            }
            vec![Effect::RedrawUrlPicker]
        }
        UrlAction::Confirm => {
            if state.entries.is_empty() {
                return vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty];
            }
            let url = state.entries[state.selected].url.clone();
            vec![
                Effect::OpenUrl(url.clone()),
                Effect::Flash(format!("Opening {url}")),
                Effect::SetMode(ViewerMode::Normal),
            ]
        }
        UrlAction::Cancel => vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::VisualLine;

    fn make_vl(md: &str, line_range: Option<(usize, usize)>) -> VisualLine {
        let md_block_range = line_range.map(|(start_line, end_line)| {
            let start_byte: usize = md
                .split('\n')
                .take(start_line - 1)
                .map(|l| l.len() + 1)
                .sum();
            let end_byte: usize = md
                .split('\n')
                .take(end_line)
                .map(|l| l.len() + 1)
                .sum::<usize>()
                .min(md.len());
            start_byte..end_byte
        });
        VisualLine {
            y_pt: 0.0,
            y_px: 0,
            md_block_range,
            md_offset: None,
        }
    }

    #[test]
    fn test_collect_all_url_entries_basic() {
        let md =
            "See [Rust](https://rust.invalid/) here.\nPlain line.\n[Docs](https://docs.invalid/)\n";
        let vls = vec![
            make_vl(md, Some((1, 1))),
            make_vl(md, Some((2, 2))),
            make_vl(md, Some((3, 3))),
        ];
        let entries = collect_all_url_entries(md, &vls);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].url, "https://rust.invalid/");
        assert_eq!(entries[0].text, "Rust");
        assert_eq!(entries[0].visual_line, 1);
        assert_eq!(entries[1].url, "https://docs.invalid/");
        assert_eq!(entries[1].visual_line, 3);
    }

    #[test]
    fn test_collect_all_url_entries_empty() {
        let md = "No links here.\n";
        let vls = vec![make_vl(md, Some((1, 1)))];
        let entries = collect_all_url_entries(md, &vls);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_collect_deduplicates_same_range() {
        let md = "See [A](https://a.invalid/) text.\n";
        // Two visual lines with the same md_block_range (can happen with multiline rendering)
        let vls = vec![make_vl(md, Some((1, 1))), make_vl(md, Some((1, 1)))];
        let entries = collect_all_url_entries(md, &vls);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_handle_select_next() {
        let entries = vec![
            UrlPickerEntry {
                url: "https://a.invalid/".into(),
                text: "A".into(),
                visual_line: 1,
            },
            UrlPickerEntry {
                url: "https://b.invalid/".into(),
                text: "B".into(),
                visual_line: 2,
            },
        ];
        let mut state = UrlPickerState::new(entries);
        assert_eq!(state.selected, 0);
        let _ = handle(UrlAction::SelectNext, &mut state, 20);
        assert_eq!(state.selected, 1);
        // Should clamp at end
        let _ = handle(UrlAction::SelectNext, &mut state, 20);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_handle_select_prev() {
        let entries = vec![
            UrlPickerEntry {
                url: "https://a.invalid/".into(),
                text: "A".into(),
                visual_line: 1,
            },
            UrlPickerEntry {
                url: "https://b.invalid/".into(),
                text: "B".into(),
                visual_line: 2,
            },
        ];
        let mut state = UrlPickerState::new(entries);
        state.selected = 1;
        let _ = handle(UrlAction::SelectPrev, &mut state, 20);
        assert_eq!(state.selected, 0);
        // Should clamp at 0
        let _ = handle(UrlAction::SelectPrev, &mut state, 20);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_handle_confirm_opens_selected() {
        let entries = vec![
            UrlPickerEntry {
                url: "https://a.invalid/".into(),
                text: "A".into(),
                visual_line: 1,
            },
            UrlPickerEntry {
                url: "https://b.invalid/".into(),
                text: "B".into(),
                visual_line: 2,
            },
        ];
        let mut state = UrlPickerState::new(entries);
        state.selected = 1;
        let effects = handle(UrlAction::Confirm, &mut state, 20);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenUrl(u) if u == "https://b.invalid/"))
        );
    }

    #[test]
    fn test_handle_cancel_returns_normal() {
        let entries = vec![UrlPickerEntry {
            url: "https://a.invalid/".into(),
            text: "A".into(),
            visual_line: 1,
        }];
        let mut state = UrlPickerState::new(entries);
        let effects = handle(UrlAction::Cancel, &mut state, 20);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
    }
}
