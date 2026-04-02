//! Log viewer mode: display log entries with scrolling, search, and yank.

use crossterm::{
    QueueableCommand, cursor,
    style::{self, Stylize},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write, stdout};

use super::Effect;
use super::effect::ScreenRestore;
use super::keymap::LogAction;
use super::layout::Layout;

/// Mutable state for log viewer mode.
pub(super) struct LogState {
    pub entries: Vec<crate::log::LogEntry>,
    pub scroll_offset: usize,
    pub search_mode: bool,
    pub search_query: String,
    pub search_matches: Vec<usize>,
    pub search_index: usize,
}

impl LogState {
    pub(super) fn new(buffer: &crate::log::LogBuffer) -> Self {
        Self {
            entries: buffer.entries(),
            scroll_offset: 0,
            search_mode: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_index: 0,
        }
    }
}

/// Handle a log viewer action, returning effects.
pub(super) fn handle(
    action: LogAction,
    state: &mut LogState,
    visible_count: usize,
    total_cols: usize,
) -> Vec<Effect> {
    match action {
        LogAction::ScrollDown => {
            let max_offset = compute_max_offset(&state.entries, total_cols, visible_count);
            state.scroll_offset = (state.scroll_offset + 1).min(max_offset);
            vec![Effect::RedrawLog]
        }
        LogAction::ScrollUp => {
            state.scroll_offset = state.scroll_offset.saturating_sub(1);
            vec![Effect::RedrawLog]
        }
        LogAction::JumpToTop => {
            state.scroll_offset = 0;
            vec![Effect::RedrawLog]
        }
        LogAction::JumpToBottom => {
            let max_offset = compute_max_offset(&state.entries, total_cols, visible_count);
            state.scroll_offset = max_offset;
            vec![Effect::RedrawLog]
        }
        LogAction::EnterSearch => {
            state.search_mode = true;
            state.search_query.clear();
            state.search_matches.clear();
            state.search_index = 0;
            vec![Effect::RedrawLog]
        }
        LogAction::Type(c) => {
            if state.search_mode {
                state.search_query.push(c);
                recompute_matches(state);
                // Auto-scroll to first match
                if let Some(&idx) = state.search_matches.first() {
                    state.scroll_offset = idx;
                    state.search_index = 0;
                }
                vec![Effect::RedrawLog]
            } else {
                vec![] // ignore typing when not in search mode
            }
        }
        LogAction::Backspace => {
            if state.search_mode {
                if state.search_query.is_empty() {
                    // Empty query + Backspace → exit search (like command mode)
                    state.search_mode = false;
                    state.search_matches.clear();
                    state.search_index = 0;
                } else {
                    state.search_query.pop();
                    recompute_matches(state);
                    if let Some(&idx) = state.search_matches.first() {
                        state.scroll_offset = idx;
                        state.search_index = 0;
                    }
                }
                vec![Effect::RedrawLog]
            } else {
                vec![]
            }
        }
        LogAction::SearchNext => {
            if !state.search_matches.is_empty() {
                state.search_index = (state.search_index + 1) % state.search_matches.len();
                state.scroll_offset = state.search_matches[state.search_index];
            }
            vec![Effect::RedrawLog]
        }
        LogAction::SearchPrev => {
            if !state.search_matches.is_empty() {
                state.search_index = if state.search_index == 0 {
                    state.search_matches.len() - 1
                } else {
                    state.search_index - 1
                };
                state.scroll_offset = state.search_matches[state.search_index];
            }
            vec![Effect::RedrawLog]
        }
        LogAction::Yank => {
            let text = state
                .entries
                .iter()
                .map(|e| e.format())
                .collect::<Vec<_>>()
                .join("\n");
            vec![Effect::Yank(text)]
        }
        LogAction::Cancel => {
            if state.search_mode {
                state.search_mode = false;
                vec![Effect::RedrawLog]
            } else {
                vec![
                    Effect::ExitToNormal(ScreenRestore::FullRefresh),
                    Effect::MarkDirty,
                ]
            }
        }
    }
}

fn recompute_matches(state: &mut LogState) {
    state.search_matches.clear();
    if state.search_query.is_empty() {
        return;
    }
    let query_lower = state.search_query.to_lowercase();
    for (i, entry) in state.entries.iter().enumerate() {
        let formatted = entry.format().to_lowercase();
        if formatted.contains(&query_lower) {
            state.search_matches.push(i);
        }
    }
}

/// Wrap a formatted entry string into display lines.
/// First line uses full width; continuation lines are indented by 2 spaces.
fn wrap_entry(formatted: &str, total_cols: usize) -> Vec<String> {
    if total_cols == 0 {
        return vec![formatted.to_string()];
    }
    let chars: Vec<char> = formatted.chars().collect();
    if chars.len() <= total_cols {
        return vec![formatted.to_string()];
    }
    let mut lines = Vec::new();
    // First line: full width
    let end = total_cols.min(chars.len());
    lines.push(chars[..end].iter().collect());
    let mut pos = end;
    // Continuation lines: 2-space indent
    const CONT_INDENT: usize = 2;
    let cont_width = total_cols.saturating_sub(CONT_INDENT);
    if cont_width == 0 {
        // Terminal too narrow for indent; wrap without indent
        while pos < chars.len() {
            let end = (pos + total_cols).min(chars.len());
            let content: String = chars[pos..end].iter().collect();
            lines.push(content);
            pos = end;
        }
    } else {
        while pos < chars.len() {
            let end = (pos + cont_width).min(chars.len());
            let content: String = chars[pos..end].iter().collect();
            lines.push(format!("  {content}"));
            pos = end;
        }
    }
    lines
}

/// Compute the maximum scroll offset accounting for wrapped line heights.
/// Finds the largest entry index such that entries from that index onward
/// fill at least `visible_count` display rows.
fn compute_max_offset(
    entries: &[crate::log::LogEntry],
    total_cols: usize,
    visible_count: usize,
) -> usize {
    if entries.is_empty() || visible_count == 0 {
        return 0;
    }
    let mut rows_used = 0;
    for i in (0..entries.len()).rev() {
        let wrapped = wrap_entry(&entries[i].format(), total_cols);
        if rows_used + wrapped.len() > visible_count {
            if rows_used == 0 {
                // Single entry exceeds visible_count — scroll to it (clipped)
                return i;
            }
            // Entries (i+1).. fill the screen; max_offset is i+1
            return i + 1;
        }
        rows_used += wrapped.len();
    }
    0
}

/// Draw the log viewer overlay screen.
pub(super) fn draw_log_screen(layout: &Layout, state: &LogState) -> io::Result<()> {
    let mut out = stdout();
    out.queue(Clear(ClearType::All))?;
    let total_cols = (layout.sidebar_cols + layout.image_cols) as usize;

    // Row 0: header
    out.queue(cursor::MoveTo(0, 0))?;
    let header = if state.search_mode {
        format!(" Log Messages: /{}", state.search_query)
    } else {
        " Log Messages:".to_string()
    };
    write!(out, "{}", header.white().bold())?;

    // Rows 1..status_row: log entries
    let list_start_row: u16 = 1;
    let list_end_row = layout.status_row;
    let match_set: std::collections::HashSet<usize> =
        state.search_matches.iter().copied().collect();
    let current_match = state.search_matches.get(state.search_index).copied();

    let mut row = list_start_row;
    let mut entry_idx = state.scroll_offset;

    while row < list_end_row && entry_idx < state.entries.len() {
        let entry = &state.entries[entry_idx];
        let formatted = entry.format();
        let wrapped = wrap_entry(&formatted, total_cols);

        let is_match = match_set.contains(&entry_idx);
        let is_current = current_match == Some(entry_idx);

        for line in &wrapped {
            if row >= list_end_row {
                break;
            }
            out.queue(cursor::MoveTo(0, row))?;
            let pad = total_cols.saturating_sub(line.chars().count());

            if is_current {
                write!(
                    out,
                    "{}",
                    format!("{line}{:pad$}", "").on_dark_blue().white()
                )?;
            } else if is_match {
                write!(
                    out,
                    "{}",
                    format!("{line}{:pad$}", "").on_dark_grey().white()
                )?;
            } else {
                match entry.level {
                    log::Level::Error => write!(out, "{}{:pad$}", line.as_str().red(), "")?,
                    log::Level::Warn => write!(out, "{}{:pad$}", line.as_str().yellow(), "")?,
                    log::Level::Debug | log::Level::Trace => {
                        write!(out, "{}{:pad$}", line.as_str().dark_grey(), "")?;
                    }
                    _ => write!(out, "{line}{:pad$}", "")?,
                }
            }
            row += 1;
        }
        entry_idx += 1;
    }

    // Clear remaining rows
    while row < list_end_row {
        out.queue(cursor::MoveTo(0, row))?;
        write!(out, "{:width$}", "", width = total_cols)?;
        row += 1;
    }

    // Status line
    out.queue(cursor::MoveTo(0, layout.status_row))?;
    let match_info = if !state.search_matches.is_empty() {
        format!(
            " [{}/{}]",
            state.search_index + 1,
            state.search_matches.len()
        )
    } else {
        String::new()
    };
    let status = format!(
        " {} entries{} | j/k:scroll g/G:top/bottom /:search y:yank q:back",
        state.entries.len(),
        match_info
    );
    let padded = format!("{:<width$}", status, width = total_cols);
    write!(out, "{}", padded.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;

    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(n: usize) -> LogState {
        let mut entries = Vec::new();
        for i in 0..n {
            entries.push(crate::log::LogEntry {
                timestamp: std::time::SystemTime::UNIX_EPOCH,
                level: log::Level::Info,
                target: "test".into(),
                message: format!("msg{i}"),
            });
        }
        LogState {
            entries,
            scroll_offset: 0,
            search_mode: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_index: 0,
        }
    }

    #[test]
    fn scroll_down_clamps() {
        let mut state = make_state(5);
        // visible_count=3 -> max_offset=2
        handle(LogAction::ScrollDown, &mut state, 3, 200);
        assert_eq!(state.scroll_offset, 1);
        handle(LogAction::ScrollDown, &mut state, 3, 200);
        assert_eq!(state.scroll_offset, 2);
        handle(LogAction::ScrollDown, &mut state, 3, 200);
        assert_eq!(state.scroll_offset, 2); // clamped
    }

    #[test]
    fn scroll_up_clamps() {
        let mut state = make_state(5);
        state.scroll_offset = 1;
        handle(LogAction::ScrollUp, &mut state, 3, 200);
        assert_eq!(state.scroll_offset, 0);
        handle(LogAction::ScrollUp, &mut state, 3, 200);
        assert_eq!(state.scroll_offset, 0); // clamped
    }

    #[test]
    fn jump_top_bottom() {
        let mut state = make_state(10);
        handle(LogAction::JumpToBottom, &mut state, 5, 200);
        assert_eq!(state.scroll_offset, 5); // 10-5
        handle(LogAction::JumpToTop, &mut state, 5, 200);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn cancel_exits_search_first() {
        let mut state = make_state(3);
        state.search_mode = true;
        let effects = handle(LogAction::Cancel, &mut state, 10, 200);
        assert!(!state.search_mode);
        assert!(effects.iter().any(|e| matches!(e, Effect::RedrawLog)));
        // Second cancel returns to Normal
        let effects = handle(LogAction::Cancel, &mut state, 10, 200);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::FullRefresh)))
        );
    }

    #[test]
    fn yank_all() {
        let mut state = make_state(2);
        let effects = handle(LogAction::Yank, &mut state, 10, 200);
        assert!(effects.iter().any(|e| matches!(e, Effect::Yank(_))));
    }

    #[test]
    fn search_finds_matches() {
        let mut state = make_state(5);
        state.entries[2].message = "special".into();
        state.entries[4].message = "special too".into();
        // Enter search, type query
        handle(LogAction::EnterSearch, &mut state, 10, 200);
        assert!(state.search_mode);
        handle(LogAction::Type('s'), &mut state, 10, 200);
        handle(LogAction::Type('p'), &mut state, 10, 200);
        handle(LogAction::Type('e'), &mut state, 10, 200);
        handle(LogAction::Type('c'), &mut state, 10, 200);
        assert_eq!(state.search_matches.len(), 2);
        assert_eq!(state.search_matches[0], 2);
        assert_eq!(state.search_matches[1], 4);
    }

    #[test]
    fn backspace_empty_query_exits_search() {
        let mut state = make_state(3);
        handle(LogAction::EnterSearch, &mut state, 10, 200);
        assert!(state.search_mode);
        // Backspace on empty query → exit search mode
        let effects = handle(LogAction::Backspace, &mut state, 10, 200);
        assert!(!state.search_mode);
        assert!(effects.iter().any(|e| matches!(e, Effect::RedrawLog)));
    }

    #[test]
    fn backspace_non_empty_query_pops() {
        let mut state = make_state(3);
        handle(LogAction::EnterSearch, &mut state, 10, 200);
        handle(LogAction::Type('a'), &mut state, 10, 200);
        handle(LogAction::Type('b'), &mut state, 10, 200);
        assert_eq!(state.search_query, "ab");
        handle(LogAction::Backspace, &mut state, 10, 200);
        assert_eq!(state.search_query, "a");
        assert!(state.search_mode); // still in search
    }

    #[test]
    fn search_next_prev_cycles() {
        let mut state = make_state(5);
        state.search_mode = true;
        state.search_matches = vec![1, 3];
        state.search_index = 0;
        handle(LogAction::SearchNext, &mut state, 10, 200);
        assert_eq!(state.search_index, 1);
        handle(LogAction::SearchNext, &mut state, 10, 200);
        assert_eq!(state.search_index, 0); // wraps
        handle(LogAction::SearchPrev, &mut state, 10, 200);
        assert_eq!(state.search_index, 1); // wraps back
    }

    // --- wrap_entry tests ---

    #[test]
    fn wrap_entry_no_wrap() {
        let lines = wrap_entry("short", 80);
        assert_eq!(lines, vec!["short"]);
    }

    #[test]
    fn wrap_entry_exact_width() {
        let s = "a".repeat(40);
        let lines = wrap_entry(&s, 40);
        assert_eq!(lines, vec![s]);
    }

    #[test]
    fn wrap_entry_wraps_with_indent() {
        // 15 chars, width 10 → first line 10, continuation 5 chars + 2 indent
        let s = "abcdefghij12345";
        let lines = wrap_entry(s, 10);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "abcdefghij");
        assert_eq!(lines[1], "  12345");
    }

    #[test]
    fn wrap_entry_multiple_continuations() {
        // 25 chars, width 10 → first 10, then 8+8 (cont_width=8)
        let s = "a".repeat(25);
        let lines = wrap_entry(&s, 10);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "a".repeat(10));
        assert_eq!(lines[1], format!("  {}", "a".repeat(8)));
        assert_eq!(lines[2], format!("  {}", "a".repeat(7)));
    }

    #[test]
    fn wrap_entry_narrow_no_indent() {
        // total_cols=2: no room for 2-space indent, falls back to no-indent wrapping
        let lines = wrap_entry("abcde", 2);
        assert_eq!(lines, vec!["ab", "cd", "e"]);

        let lines = wrap_entry("abc", 1);
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn wrap_entry_zero_width() {
        let lines = wrap_entry("hello", 0);
        assert_eq!(lines, vec!["hello"]);
    }

    // --- compute_max_offset tests ---

    #[test]
    fn compute_max_offset_no_wrap() {
        // 5 short entries, visible_count=3 → max_offset=2
        let state = make_state(5);
        let max = compute_max_offset(&state.entries, 200, 3);
        assert_eq!(max, 2);
    }

    #[test]
    fn compute_max_offset_all_fit() {
        let state = make_state(3);
        let max = compute_max_offset(&state.entries, 200, 10);
        assert_eq!(max, 0);
    }

    #[test]
    fn compute_max_offset_with_long_entry() {
        let mut state = make_state(5);
        // Make the last entry very long so it wraps to many lines
        state.entries[4].message = "x".repeat(200);
        let formatted_len = state.entries[4].format().chars().count();
        // First line: 30 chars, continuation lines: 28 chars each
        let expected_lines = 1 + (formatted_len - 30).div_ceil(28);
        assert!(expected_lines > 3); // sanity: wraps to more than visible_count
        // visible_count=3: last entry alone exceeds 3 rows, so max_offset=4
        let max = compute_max_offset(&state.entries, 30, 3);
        assert_eq!(max, 4);
    }

    #[test]
    fn scroll_clamps_with_wrapping() {
        let mut state = make_state(3);
        state.entries[2].message = "x".repeat(200); // wraps to many lines
        // visible_count=5, total_cols=30
        // Entry 2 alone takes many rows, so max_offset should account for that
        let max = compute_max_offset(&state.entries, 30, 5);
        // Scroll to bottom
        handle(LogAction::JumpToBottom, &mut state, 5, 30);
        assert_eq!(state.scroll_offset, max);
    }
}
