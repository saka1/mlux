//! Log viewer mode: display log entries with scrolling, search, and yank.

use crossterm::{
    QueueableCommand, cursor,
    style::{self, Stylize},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write, stdout};

use super::keymap::LogAction;
use super::layout::Layout;
use super::{Effect, ViewerMode};

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
pub(super) fn handle(action: LogAction, state: &mut LogState, visible_count: usize) -> Vec<Effect> {
    match action {
        LogAction::ScrollDown => {
            let max_offset = state.entries.len().saturating_sub(visible_count);
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
            let max_offset = state.entries.len().saturating_sub(visible_count);
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
                vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty]
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
    let visible_count = (list_end_row - list_start_row) as usize;

    let match_set: std::collections::HashSet<usize> =
        state.search_matches.iter().copied().collect();
    let current_match = state.search_matches.get(state.search_index).copied();

    for i in 0..visible_count {
        let entry_idx = state.scroll_offset + i;
        let row = list_start_row + i as u16;
        out.queue(cursor::MoveTo(0, row))?;

        if entry_idx >= state.entries.len() {
            write!(out, "{:width$}", "", width = total_cols)?;
            continue;
        }

        let entry = &state.entries[entry_idx];
        let formatted = entry.format();
        let display: String = formatted.chars().take(total_cols).collect();
        let pad = total_cols.saturating_sub(display.len());

        let is_match = match_set.contains(&entry_idx);
        let is_current = current_match == Some(entry_idx);

        // Color by level, highlight matches
        if is_current {
            write!(
                out,
                "{}",
                format!("{display}{:pad$}", "").on_dark_blue().white()
            )?;
        } else if is_match {
            write!(
                out,
                "{}",
                format!("{display}{:pad$}", "").on_dark_grey().white()
            )?;
        } else {
            match entry.level {
                log::Level::Error => write!(out, "{}{:pad$}", display.red(), "")?,
                log::Level::Warn => write!(out, "{}{:pad$}", display.yellow(), "")?,
                log::Level::Debug | log::Level::Trace => {
                    write!(out, "{}{:pad$}", display.dark_grey(), "")?;
                }
                _ => write!(out, "{display}{:pad$}", "")?, // Info = default
            }
        }
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
        handle(LogAction::ScrollDown, &mut state, 3);
        assert_eq!(state.scroll_offset, 1);
        handle(LogAction::ScrollDown, &mut state, 3);
        assert_eq!(state.scroll_offset, 2);
        handle(LogAction::ScrollDown, &mut state, 3);
        assert_eq!(state.scroll_offset, 2); // clamped
    }

    #[test]
    fn scroll_up_clamps() {
        let mut state = make_state(5);
        state.scroll_offset = 1;
        handle(LogAction::ScrollUp, &mut state, 3);
        assert_eq!(state.scroll_offset, 0);
        handle(LogAction::ScrollUp, &mut state, 3);
        assert_eq!(state.scroll_offset, 0); // clamped
    }

    #[test]
    fn jump_top_bottom() {
        let mut state = make_state(10);
        handle(LogAction::JumpToBottom, &mut state, 5);
        assert_eq!(state.scroll_offset, 5); // 10-5
        handle(LogAction::JumpToTop, &mut state, 5);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn cancel_exits_search_first() {
        let mut state = make_state(3);
        state.search_mode = true;
        let effects = handle(LogAction::Cancel, &mut state, 10);
        assert!(!state.search_mode);
        assert!(effects.iter().any(|e| matches!(e, Effect::RedrawLog)));
        // Second cancel returns to Normal
        let effects = handle(LogAction::Cancel, &mut state, 10);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
    }

    #[test]
    fn yank_all() {
        let mut state = make_state(2);
        let effects = handle(LogAction::Yank, &mut state, 10);
        assert!(effects.iter().any(|e| matches!(e, Effect::Yank(_))));
    }

    #[test]
    fn search_finds_matches() {
        let mut state = make_state(5);
        state.entries[2].message = "special".into();
        state.entries[4].message = "special too".into();
        // Enter search, type query
        handle(LogAction::EnterSearch, &mut state, 10);
        assert!(state.search_mode);
        handle(LogAction::Type('s'), &mut state, 10);
        handle(LogAction::Type('p'), &mut state, 10);
        handle(LogAction::Type('e'), &mut state, 10);
        handle(LogAction::Type('c'), &mut state, 10);
        assert_eq!(state.search_matches.len(), 2);
        assert_eq!(state.search_matches[0], 2);
        assert_eq!(state.search_matches[1], 4);
    }

    #[test]
    fn backspace_empty_query_exits_search() {
        let mut state = make_state(3);
        handle(LogAction::EnterSearch, &mut state, 10);
        assert!(state.search_mode);
        // Backspace on empty query → exit search mode
        let effects = handle(LogAction::Backspace, &mut state, 10);
        assert!(!state.search_mode);
        assert!(effects.iter().any(|e| matches!(e, Effect::RedrawLog)));
    }

    #[test]
    fn backspace_non_empty_query_pops() {
        let mut state = make_state(3);
        handle(LogAction::EnterSearch, &mut state, 10);
        handle(LogAction::Type('a'), &mut state, 10);
        handle(LogAction::Type('b'), &mut state, 10);
        assert_eq!(state.search_query, "ab");
        handle(LogAction::Backspace, &mut state, 10);
        assert_eq!(state.search_query, "a");
        assert!(state.search_mode); // still in search
    }

    #[test]
    fn search_next_prev_cycles() {
        let mut state = make_state(5);
        state.search_mode = true;
        state.search_matches = vec![1, 3];
        state.search_index = 0;
        handle(LogAction::SearchNext, &mut state, 10);
        assert_eq!(state.search_index, 1);
        handle(LogAction::SearchNext, &mut state, 10);
        assert_eq!(state.search_index, 0); // wraps
        handle(LogAction::SearchPrev, &mut state, 10);
        assert_eq!(state.search_index, 1); // wraps back
    }
}
