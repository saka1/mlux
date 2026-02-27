//! Search functionality: grep Markdown source and display results as a picker.

use crossterm::{
    QueueableCommand,
    cursor,
    style::{self, Stylize},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write, stdout};

use super::state::Layout;
use crate::tile::VisualLine;

/// A single search match within the Markdown source.
#[derive(Debug, Clone)]
pub(super) struct SearchMatch {
    /// 1-based Markdown line number.
    pub md_line: usize,
    /// Index into the `visual_lines` array (for jumping).
    pub visual_line_idx: usize,
    /// The full text of the matching line.
    pub context: String,
    /// Byte offset of match start within `context`.
    pub col_start: usize,
    /// Byte offset of match end within `context`.
    pub col_end: usize,
}

/// Mutable search state while in search mode.
pub(super) struct SearchState {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub selected: usize,
    pub scroll_offset: usize,
}

impl SearchState {
    pub(super) fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            scroll_offset: 0,
        }
    }
}

/// Persisted search results for n/N navigation in normal mode.
pub(super) struct LastSearch {
    pub matches: Vec<SearchMatch>,
    pub current_idx: usize,
}

impl LastSearch {
    /// Create from a completed SearchState, using the selected match as current.
    pub(super) fn from_search_state(ss: &SearchState) -> Self {
        Self {
            matches: ss.matches.clone(),
            current_idx: ss.selected,
        }
    }

    /// Advance to the next match. Wraps around.
    pub(super) fn advance_next(&mut self) {
        if !self.matches.is_empty() {
            self.current_idx = (self.current_idx + 1) % self.matches.len();
        }
    }

    /// Advance to the previous match. Wraps around.
    pub(super) fn advance_prev(&mut self) {
        if !self.matches.is_empty() {
            if self.current_idx == 0 {
                self.current_idx = self.matches.len() - 1;
            } else {
                self.current_idx -= 1;
            }
        }
    }

    /// Get the visual_line_idx of the current match.
    pub(super) fn current_visual_line_idx(&self) -> Option<usize> {
        self.matches.get(self.current_idx).map(|m| m.visual_line_idx)
    }
}

/// Find the visual line index corresponding to a 1-based Markdown line number.
fn find_visual_line(visual_lines: &[VisualLine], md_line: usize) -> Option<usize> {
    visual_lines.iter().position(|vl| {
        vl.md_line_range
            .is_some_and(|(s, e)| md_line >= s && md_line <= e)
    })
}

/// Search the Markdown source for lines matching `query`.
///
/// Uses smartcase: if `query` is all lowercase, search is case-insensitive;
/// otherwise it's case-sensitive.
pub(super) fn grep_markdown(
    query: &str,
    markdown: &str,
    visual_lines: &[VisualLine],
) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let smartcase = query.chars().all(|c| !c.is_uppercase());
    let query_lower = if smartcase {
        query.to_lowercase()
    } else {
        String::new()
    };

    let mut matches = Vec::new();

    for (line_idx, line_text) in markdown.lines().enumerate() {
        let md_line = line_idx + 1; // 1-based

        // Find match position
        let found = if smartcase {
            let line_lower = line_text.to_lowercase();
            find_match_pos(&line_lower, &query_lower)
        } else {
            find_match_pos(line_text, query)
        };

        if let Some((col_start, col_end)) = found {
            // Map the byte offsets back to the original text.
            // For smartcase, the byte positions may differ between lowered and original.
            // Recompute on original text to get correct byte offsets.
            let (actual_start, actual_end) = if smartcase {
                // Re-find in original using char-level comparison
                find_match_pos_caseless(line_text, query).unwrap_or((col_start, col_end))
            } else {
                (col_start, col_end)
            };

            if let Some(vl_idx) = find_visual_line(visual_lines, md_line) {
                matches.push(SearchMatch {
                    md_line,
                    visual_line_idx: vl_idx,
                    context: line_text.to_string(),
                    col_start: actual_start,
                    col_end: actual_end,
                });
            }
        }
    }

    matches
}

/// Find the byte position of the first occurrence of `needle` in `haystack`.
fn find_match_pos(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    haystack.find(needle).map(|start| (start, start + needle.len()))
}

/// Case-insensitive match position finder using char-level comparison.
fn find_match_pos_caseless(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    let needle_chars: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).collect();
    let needle_len = needle_chars.len();
    if needle_len == 0 {
        return None;
    }

    let haystack_chars: Vec<(usize, char)> = haystack.char_indices().collect();
    for i in 0..haystack_chars.len() {
        if i + needle_len > haystack_chars.len() {
            break;
        }
        let mut matched = true;
        for j in 0..needle_len {
            let hay_lower: Vec<char> = haystack_chars[i + j].1.to_lowercase().collect();
            if hay_lower.len() != 1 || hay_lower[0] != needle_chars[j] {
                matched = false;
                break;
            }
        }
        if matched {
            let start = haystack_chars[i].0;
            let end = if i + needle_len < haystack_chars.len() {
                haystack_chars[i + needle_len].0
            } else {
                haystack.len()
            };
            return Some((start, end));
        }
    }
    None
}

/// Truncate a string to at most `max_bytes`, respecting UTF-8 char boundaries.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    &s[..s.floor_char_boundary(max_bytes)]
}

/// Draw the search screen (replaces tile images).
///
/// Layout:
///   Row 0: `/query_` prompt
///   Row 1..N: result list (scrolled, with selection highlight)
///   Last row: status line with match count and key hints
pub(super) fn draw_search_screen(
    layout: &Layout,
    query: &str,
    matches: &[SearchMatch],
    selected: usize,
    scroll_offset: usize,
) -> io::Result<()> {
    let mut out = stdout();
    out.queue(Clear(ClearType::All))?;

    let total_cols = (layout.sidebar_cols + layout.image_cols) as usize;

    // Row 0: search prompt
    out.queue(cursor::MoveTo(0, 0))?;
    let prompt = format!("/{query}_");
    let prompt_display = truncate_str(&prompt, total_cols);
    write!(out, "{}", prompt_display.white().bold())?;

    // Result list: rows 1 .. status_row-1
    let list_start_row: u16 = 1;
    let list_end_row = layout.status_row; // exclusive
    let visible_count = (list_end_row - list_start_row) as usize;

    for i in 0..visible_count {
        let match_idx = scroll_offset + i;
        let row = list_start_row + i as u16;
        out.queue(cursor::MoveTo(0, row))?;

        if match_idx >= matches.len() {
            // Empty row
            write!(out, "{:width$}", "", width = total_cols)?;
            continue;
        }

        let m = &matches[match_idx];
        let is_selected = match_idx == selected;

        // Format: "  {line_num}: {context}" with match highlight
        let line_prefix = format!("  {:>4}: ", m.md_line);
        let prefix_len = line_prefix.len();

        // Truncate context to fit, respecting UTF-8 boundaries
        let max_context = total_cols.saturating_sub(prefix_len);
        let context = truncate_str(&m.context, max_context);

        // Clamp highlight offsets to truncated context length and char boundaries
        let col_start = context.floor_char_boundary(m.col_start.min(context.len()));
        let col_end = context.floor_char_boundary(m.col_end.min(context.len()));

        let before = &context[..col_start];
        let highlight = &context[col_start..col_end];
        let after = &context[col_end..];

        if is_selected {
            // Selected line: full line on blue background
            write!(out, "{}", line_prefix.on_dark_blue().white())?;
            write!(out, "{}", before.on_dark_blue().white())?;
            write!(out, "{}", highlight.on_dark_blue().yellow().bold())?;
            // Pad remaining
            let remaining = total_cols.saturating_sub(prefix_len + context.len());
            write!(out, "{}", format!("{after}{:remaining$}", "").on_dark_blue().white())?;
        } else {
            // Normal line: highlight match in reverse
            write!(out, "{}", line_prefix.dark_grey())?;
            write!(out, "{before}")?;
            write!(out, "{}", highlight.yellow().bold())?;
            write!(out, "{after}")?;
        }
    }

    // Status line
    out.queue(cursor::MoveTo(0, layout.status_row))?;
    let status = format!(
        " {} matches | Enter:jump  Esc:cancel  j/k:select",
        matches.len()
    );
    let padded = format!("{:<width$}", status, width = total_cols);
    write!(out, "{}", padded.on_dark_grey().white())?;
    out.queue(style::ResetColor)?;

    out.flush()
}
