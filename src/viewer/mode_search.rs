//! Search functionality: grep Markdown source and display results as a picker.

use crossterm::{
    QueueableCommand, cursor,
    style::{self, Stylize},
    terminal::{Clear, ClearType},
};
use regex::RegexBuilder;
use std::io::{self, Write, stdout};

use super::Effect;
use super::ViewerMode;
use super::input::SearchAction;
use super::layout::{Layout, visual_line_offset};
use super::query::DocumentQuery;

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
    /// Byte range of the match within the full Markdown source.
    pub md_range: std::ops::Range<usize>,
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
    /// All target_ranges (union of all matches).
    target_ranges: Vec<std::ops::Range<usize>>,
    /// Per-match main.typ byte ranges, indexed by match position in the
    /// Markdown source. Used to populate `active_ranges` for the current match.
    per_match_ranges: Vec<Vec<std::ops::Range<usize>>>,
}

impl LastSearch {
    /// Create from a completed SearchState, using the selected match as current.
    ///
    /// Computes per-match ranges and caches the combined target_ranges.
    pub(super) fn from_search_state(ss: &SearchState, doc: &DocumentQuery) -> Self {
        let all_md_ranges: Vec<std::ops::Range<usize>> =
            ss.matches.iter().map(|m| m.md_range.clone()).collect();

        let per_match_ranges: Vec<Vec<std::ops::Range<usize>>> = all_md_ranges
            .iter()
            .map(|r| {
                doc.content_index.md_to_main_ranges(
                    std::slice::from_ref(r),
                    doc.markdown,
                    doc.content_offset,
                )
            })
            .collect();

        let target_ranges =
            doc.content_index
                .md_to_main_ranges(&all_md_ranges, doc.markdown, doc.content_offset);

        Self {
            matches: ss.matches.clone(),
            current_idx: ss.selected,
            target_ranges,
            per_match_ranges,
        }
    }

    /// Build a highlight spec with `active_ranges` set to the current match.
    pub(super) fn highlight_spec(&self) -> crate::highlight::HighlightSpec {
        let active_ranges = self
            .per_match_ranges
            .get(self.current_idx)
            .cloned()
            .unwrap_or_default();
        crate::highlight::HighlightSpec {
            target_ranges: self.target_ranges.clone(),
            active_ranges,
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

/// Search the Markdown source for lines matching `query` as a regular expression.
///
/// Uses smartcase: if `query` is all lowercase, search is case-insensitive;
/// otherwise it's case-sensitive.
///
/// Returns `(matches, pattern_valid)`. On invalid regex, returns empty matches
/// with `pattern_valid = false`.
pub(super) fn grep_markdown(doc: &DocumentQuery, query: &str) -> (Vec<SearchMatch>, bool) {
    if query.is_empty() {
        return (Vec::new(), true);
    }

    let smartcase = query.chars().all(|c| !c.is_uppercase());
    let re = match RegexBuilder::new(query).case_insensitive(smartcase).build() {
        Ok(re) => re,
        Err(_) => return (Vec::new(), false),
    };

    let mut matches = Vec::new();
    let mut line_byte_offset = 0usize;

    // Split on '\n' rather than using lines() so that line_byte_offset tracks
    // the exact byte position in the original string (matching md_block_range).
    // Trailing '\r' (CRLF) is stripped only for regex matching and display.
    for (line_idx, raw_chunk) in doc.markdown.split('\n').enumerate() {
        let line_text = raw_chunk.trim_end_matches('\r');
        let md_line = line_idx + 1; // 1-based

        if let Some(m) = re.find(line_text)
            && let Some(vl_idx) = doc.find_visual_line_by_offset(line_byte_offset + m.start())
        {
            matches.push(SearchMatch {
                md_line,
                visual_line_idx: vl_idx,
                context: line_text.to_string(),
                col_start: m.start(),
                col_end: m.end(),
                md_range: line_byte_offset + m.start()..line_byte_offset + m.end(),
            });
        }

        line_byte_offset += raw_chunk.len() + 1; // +1 for the '\n' we split on
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

        // Alt+N shortcut label: "[N] " for items 1-9, "    " otherwise
        let label = if i < 9 {
            format!("[{}] ", i + 1)
        } else {
            "    ".to_string()
        };

        // Format: "{label}  {line_num}: {context}" with match highlight
        let line_prefix = format!("{label}  {:>4}: ", m.md_line);
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
            " {} matches | Enter:jump  Esc:cancel  ↑↓:select  Alt+1-9:jump",
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
    doc: &DocumentQuery,
    visible_count: usize,
    max_scroll: u32,
) -> Vec<Effect> {
    match action {
        SearchAction::Type(c) => {
            ss.query.push(c);
            update_grep(ss, doc);
            vec![Effect::RedrawSearch]
        }
        SearchAction::Backspace => {
            if ss.query.is_empty() {
                return vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty];
            }
            ss.query.pop();
            update_grep(ss, doc);
            vec![Effect::RedrawSearch]
        }
        SearchAction::SelectNext => {
            if !ss.matches.is_empty() {
                ss.selected = (ss.selected + 1).min(ss.matches.len() - 1);
                if ss.selected >= ss.scroll_offset + visible_count {
                    ss.scroll_offset = ss.selected - visible_count + 1;
                }
            }
            vec![Effect::RedrawSearch]
        }
        SearchAction::SelectPrev => {
            if !ss.matches.is_empty() {
                ss.selected = ss.selected.saturating_sub(1);
                if ss.selected < ss.scroll_offset {
                    ss.scroll_offset = ss.selected;
                }
            }
            vec![Effect::RedrawSearch]
        }
        SearchAction::SelectIndex(n) => {
            let target = ss.scroll_offset + n;
            if target >= ss.matches.len() {
                return vec![Effect::RedrawSearch];
            }
            ss.selected = target;
            let vl_idx = ss.matches[ss.selected].visual_line_idx;
            let last = LastSearch::from_search_state(ss, doc);
            let line_num = (vl_idx + 1) as u32;
            let y = visual_line_offset(doc.visual_lines, max_scroll, line_num);
            let flash = format!("match {}/{}", ss.selected + 1, ss.matches.len());
            vec![
                Effect::SetLastSearch(last),
                Effect::ScrollTo(y),
                Effect::Flash(flash),
                Effect::SetMode(ViewerMode::Normal),
            ]
        }
        SearchAction::Confirm => {
            if ss.matches.is_empty() {
                return vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty];
            }
            let vl_idx = ss.matches[ss.selected].visual_line_idx;
            let last = LastSearch::from_search_state(ss, doc);
            let line_num = (vl_idx + 1) as u32;
            let y = visual_line_offset(doc.visual_lines, max_scroll, line_num);
            let flash = format!("match {}/{}", ss.selected + 1, ss.matches.len());
            vec![
                Effect::SetLastSearch(last),
                Effect::ScrollTo(y),
                Effect::Flash(flash),
                Effect::SetMode(ViewerMode::Normal),
            ]
        }
        SearchAction::Cancel => vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty],
    }
}

/// Re-run grep on the current query and reset selection.
fn update_grep(ss: &mut SearchState, doc: &DocumentQuery) {
    let (matches, valid) = grep_markdown(doc, &ss.query);
    ss.matches = matches;
    ss.pattern_valid = valid;
    ss.selected = 0;
    ss.scroll_offset = 0;
}

#[cfg(test)]
mod tests {
    use super::super::query::test_helpers::*;
    use super::*;

    #[test]
    fn regex_heading_pattern() {
        let md = "# Title\nsome text\n## Subtitle\nmore text";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let (matches, valid) = grep_markdown(&doc, "^#");
        assert!(valid);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].md_line, 1);
        assert_eq!(matches[1].md_line, 3);
    }

    #[test]
    fn smartcase_all_lower_is_insensitive() {
        let md = "Hello World\nhello world\nHELLO";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let (matches, valid) = grep_markdown(&doc, "hello");
        assert!(valid);
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn smartcase_upper_is_sensitive() {
        let md = "Hello World\nhello world\nHELLO";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let (matches, valid) = grep_markdown(&doc, "Hello");
        assert!(valid);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].md_line, 1);
    }

    #[test]
    fn invalid_pattern_returns_empty() {
        let md = "some [text] here";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let (matches, valid) = grep_markdown(&doc, "[");
        assert!(!valid);
        assert!(matches.is_empty());
    }

    #[test]
    fn literal_string_still_works() {
        let md = "foo bar baz\nqux foo quux";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let (matches, valid) = grep_markdown(&doc, "foo");
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
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let (matches, valid) = grep_markdown(&doc, "");
        assert!(valid);
        assert!(matches.is_empty());
    }

    #[test]
    fn grep_markdown_stores_md_range() {
        let md = "foo bar baz\nqux foo quux";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let (matches, valid) = grep_markdown(&doc, "foo");
        assert!(valid);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].md_range, 0..3);
        assert_eq!(matches[1].md_range, 16..19);
    }

    #[test]
    fn last_search_per_match_ranges_aligned_with_matches() {
        // "foo foo bar" has two "foo" on the same line.
        // grep_markdown finds 1 match (first per line).
        // Old build_highlight_ranges would find 2 → index mismatch.
        // After fix: per_match_ranges.len() == matches.len().
        let md = "foo foo bar";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        ss.query = "foo".into();
        let (matches, _) = grep_markdown(&doc, &ss.query);
        ss.matches = matches;

        let ls = LastSearch::from_search_state(&ss, &doc);
        assert_eq!(ls.per_match_ranges.len(), ls.matches.len());
    }

    // --- handle() tests (pure, no I/O) ---

    #[test]
    fn handle_type_updates_query_and_redraws() {
        let md = "hello world\nfoo bar";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        let effects = handle(SearchAction::Type('h'), &mut ss, &doc, 20, 1000);
        assert_eq!(ss.query, "h");
        assert!(!ss.matches.is_empty()); // "hello" matches
        assert!(matches!(effects[0], Effect::RedrawSearch));
    }

    #[test]
    fn handle_backspace_pops_query() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        ss.query = "he".into();
        let effects = handle(SearchAction::Backspace, &mut ss, &doc, 20, 1000);
        assert_eq!(ss.query, "h");
        assert!(matches!(effects[0], Effect::RedrawSearch));
    }

    #[test]
    fn handle_backspace_on_empty_exits_search() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        let effects = handle(SearchAction::Backspace, &mut ss, &doc, 20, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkDirty)));
    }

    #[test]
    fn handle_select_next_moves_selection() {
        let md = "aaa\naaa\naaa";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        // Pre-populate with matches
        handle(SearchAction::Type('a'), &mut ss, &doc, 20, 1000);
        assert_eq!(ss.selected, 0);
        handle(SearchAction::SelectNext, &mut ss, &doc, 20, 1000);
        assert_eq!(ss.selected, 1);
    }

    #[test]
    fn handle_select_prev_clamps_at_zero() {
        let md = "aaa\naaa";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        handle(SearchAction::Type('a'), &mut ss, &doc, 20, 1000);
        assert_eq!(ss.selected, 0);
        handle(SearchAction::SelectPrev, &mut ss, &doc, 20, 1000);
        assert_eq!(ss.selected, 0);
    }

    #[test]
    fn handle_confirm_sets_last_search_and_scrolls() {
        let md = "hello world";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        handle(SearchAction::Type('h'), &mut ss, &doc, 20, 1000);
        let effects = handle(SearchAction::Confirm, &mut ss, &doc, 20, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetLastSearch(_)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::ScrollTo(_))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
    }

    #[test]
    fn handle_confirm_empty_returns_normal() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        // No matches (empty query)
        let effects = handle(SearchAction::Confirm, &mut ss, &doc, 20, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkDirty)));
    }

    #[test]
    fn handle_cancel_returns_normal() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        let effects = handle(SearchAction::Cancel, &mut ss, &doc, 20, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkDirty)));
    }

    #[test]
    fn handle_select_next_scrolls_when_past_visible() {
        let md = "a\nb\nc\nd\ne";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut ss = SearchState::new();
        // Search with visible_count = 2 (only 2 rows visible)
        handle(SearchAction::Type('a'), &mut ss, &doc, 2, 1000);
        // This matches only line 1, so we need a query that matches all
        ss.query.clear();
        handle(SearchAction::Type('.'), &mut ss, &doc, 2, 1000);
        assert_eq!(ss.matches.len(), 5);
        handle(SearchAction::SelectNext, &mut ss, &doc, 2, 1000);
        handle(SearchAction::SelectNext, &mut ss, &doc, 2, 1000);
        // selected=2, visible_count=2, should scroll
        assert!(ss.scroll_offset > 0);
    }
}
