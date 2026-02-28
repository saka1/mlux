//! Search functionality: grep Markdown source and display results as a picker.

use crossterm::{
    QueueableCommand, cursor,
    style::{self, Stylize},
    terminal::{Clear, ClearType},
};
use regex::RegexBuilder;
use std::io::{self, Write, stdout};

use super::input::SearchAction;
use super::state::{Layout, visual_line_offset};
use super::{Effect, ViewerMode};
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
    pub pattern_valid: bool,
}

impl SearchState {
    pub(super) fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            pattern_valid: true,
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
        self.matches
            .get(self.current_idx)
            .map(|m| m.visual_line_idx)
    }
}

/// Find the visual line index corresponding to a 1-based Markdown line number.
fn find_visual_line(visual_lines: &[VisualLine], md_line: usize) -> Option<usize> {
    visual_lines.iter().position(|vl| {
        vl.md_line_range
            .is_some_and(|(s, e)| md_line >= s && md_line <= e)
    })
}

/// Search the Markdown source for lines matching `query` as a regular expression.
///
/// Uses smartcase: if `query` is all lowercase, search is case-insensitive;
/// otherwise it's case-sensitive.
///
/// Returns `(matches, pattern_valid)`. On invalid regex, returns empty matches
/// with `pattern_valid = false`.
pub(super) fn grep_markdown(
    query: &str,
    markdown: &str,
    visual_lines: &[VisualLine],
) -> (Vec<SearchMatch>, bool) {
    if query.is_empty() {
        return (Vec::new(), true);
    }

    let smartcase = query.chars().all(|c| !c.is_uppercase());
    let re = match RegexBuilder::new(query).case_insensitive(smartcase).build() {
        Ok(re) => re,
        Err(_) => return (Vec::new(), false),
    };

    let mut matches = Vec::new();

    for (line_idx, line_text) in markdown.lines().enumerate() {
        let md_line = line_idx + 1; // 1-based

        if let Some(m) = re.find(line_text)
            && let Some(vl_idx) = find_visual_line(visual_lines, md_line)
        {
            matches.push(SearchMatch {
                md_line,
                visual_line_idx: vl_idx,
                context: line_text.to_string(),
                col_start: m.start(),
                col_end: m.end(),
            });
        }
    }

    (matches, true)
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
    pattern_valid: bool,
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
            write!(
                out,
                "{}",
                format!("{after}{:remaining$}", "").on_dark_blue().white()
            )?;
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
    let status = if !pattern_valid {
        " invalid pattern | Esc:cancel".to_string()
    } else {
        format!(
            " {} matches | Enter:jump  Esc:cancel  j/k:select",
            matches.len()
        )
    };
    let padded = format!("{:<width$}", status, width = total_cols);
    if !pattern_valid {
        write!(out, "{}", padded.on_dark_red().white())?;
    } else {
        write!(out, "{}", padded.on_dark_grey().white())?;
    }
    out.queue(style::ResetColor)?;

    out.flush()
}

pub(super) fn handle(
    action: SearchAction,
    ss: &mut SearchState,
    markdown: &str,
    visual_lines: &[VisualLine],
    layout: &Layout,
    max_scroll: u32,
) -> io::Result<Vec<Effect>> {
    match action {
        SearchAction::Type(c) => {
            ss.query.push(c);
            re_grep_and_redraw(ss, markdown, visual_lines, layout)
        }
        SearchAction::Backspace => {
            ss.query.pop();
            re_grep_and_redraw(ss, markdown, visual_lines, layout)
        }
        SearchAction::SelectNext => {
            if !ss.matches.is_empty() {
                ss.selected = (ss.selected + 1).min(ss.matches.len() - 1);
                let visible_count = (layout.status_row - 1) as usize;
                if ss.selected >= ss.scroll_offset + visible_count {
                    ss.scroll_offset = ss.selected - visible_count + 1;
                }
            }
            redraw_search(ss, layout)
        }
        SearchAction::SelectPrev => {
            if !ss.matches.is_empty() {
                ss.selected = ss.selected.saturating_sub(1);
                if ss.selected < ss.scroll_offset {
                    ss.scroll_offset = ss.selected;
                }
            }
            redraw_search(ss, layout)
        }
        SearchAction::Confirm => {
            if ss.matches.is_empty() {
                return Ok(vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty]);
            }
            let vl_idx = ss.matches[ss.selected].visual_line_idx;
            let last = LastSearch::from_search_state(ss);
            let line_num = (vl_idx + 1) as u32;
            let y = visual_line_offset(visual_lines, max_scroll, line_num);
            let flash = format!("match {}/{}", ss.selected + 1, ss.matches.len());
            Ok(vec![
                Effect::SetLastSearch(last),
                Effect::ScrollTo(y),
                Effect::Flash(flash),
                Effect::SetMode(ViewerMode::Normal),
            ])
        }
        SearchAction::Cancel => Ok(vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty]),
    }
}

/// Re-run grep on the current query, reset selection, and redraw.
fn re_grep_and_redraw(
    ss: &mut SearchState,
    markdown: &str,
    visual_lines: &[VisualLine],
    layout: &Layout,
) -> io::Result<Vec<Effect>> {
    let (matches, valid) = grep_markdown(&ss.query, markdown, visual_lines);
    ss.matches = matches;
    ss.pattern_valid = valid;
    ss.selected = 0;
    ss.scroll_offset = 0;
    redraw_search(ss, layout)
}

/// Redraw the search screen from current state and return empty effects.
fn redraw_search(ss: &SearchState, layout: &Layout) -> io::Result<Vec<Effect>> {
    draw_search_screen(
        layout,
        &ss.query,
        &ss.matches,
        ss.selected,
        ss.scroll_offset,
        ss.pattern_valid,
    )?;
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build visual_lines where line N maps to md_line_range (N, N).
    fn make_visual_lines(n: usize) -> Vec<VisualLine> {
        (1..=n)
            .map(|i| VisualLine {
                y_pt: 0.0,
                y_px: 0,
                md_line_range: Some((i, i)),
                md_line_exact: None,
            })
            .collect()
    }

    #[test]
    fn regex_heading_pattern() {
        let md = "# Title\nsome text\n## Subtitle\nmore text";
        let vl = make_visual_lines(4);
        let (matches, valid) = grep_markdown("^#", md, &vl);
        assert!(valid);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].md_line, 1);
        assert_eq!(matches[1].md_line, 3);
    }

    #[test]
    fn smartcase_all_lower_is_insensitive() {
        let md = "Hello World\nhello world\nHELLO";
        let vl = make_visual_lines(3);
        let (matches, valid) = grep_markdown("hello", md, &vl);
        assert!(valid);
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn smartcase_upper_is_sensitive() {
        let md = "Hello World\nhello world\nHELLO";
        let vl = make_visual_lines(3);
        let (matches, valid) = grep_markdown("Hello", md, &vl);
        assert!(valid);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].md_line, 1);
    }

    #[test]
    fn invalid_pattern_returns_empty() {
        let md = "some [text] here";
        let vl = make_visual_lines(1);
        let (matches, valid) = grep_markdown("[", md, &vl);
        assert!(!valid);
        assert!(matches.is_empty());
    }

    #[test]
    fn literal_string_still_works() {
        let md = "foo bar baz\nqux foo quux";
        let vl = make_visual_lines(2);
        let (matches, valid) = grep_markdown("foo", md, &vl);
        assert!(valid);
        assert_eq!(matches.len(), 2);
        // Check highlight positions
        assert_eq!(matches[0].col_start, 0);
        assert_eq!(matches[0].col_end, 3);
        assert_eq!(matches[1].col_start, 4);
        assert_eq!(matches[1].col_end, 7);
    }

    #[test]
    fn empty_query_returns_empty() {
        let md = "anything";
        let vl = make_visual_lines(1);
        let (matches, valid) = grep_markdown("", md, &vl);
        assert!(valid);
        assert!(matches.is_empty());
    }
}
