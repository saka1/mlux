//! Inline (less-style) search mode: incremental search with status bar prompt.

use super::Effect;
use super::effect::ScreenRestore;
use super::keymap::InlineSearchAction;
use super::layout::visual_line_offset;
use super::mode_grep::{LastSearch, SearchDirection, SearchMatch, grep_markdown};
use super::query::DocumentQuery;

/// State for inline search mode (`/` or `?` prompt in status bar).
pub(super) struct InlineSearchState {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub current_idx: usize,
    /// Scroll position before search started (restored on Esc).
    pub pre_search_y: u32,
    /// Search direction: Forward (`/`) or Backward (`?`).
    pub direction: SearchDirection,
}

impl InlineSearchState {
    pub(super) fn new(pre_search_y: u32, direction: SearchDirection) -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current_idx: 0,
            pre_search_y,
            direction,
        }
    }

    /// Build a highlight spec for the current search state.
    ///
    /// Used to highlight matches on tiles while searching.
    pub(super) fn highlight_spec(
        &self,
        doc: &super::query::DocumentQuery,
    ) -> crate::frame::HighlightSpec {
        let all_md_ranges: Vec<std::ops::Range<usize>> =
            self.matches.iter().map(|m| m.md_range.clone()).collect();

        let target_ranges =
            doc.content_index
                .md_to_main_ranges(&all_md_ranges, doc.markdown, doc.content_offset);

        let active_ranges = self
            .matches
            .get(self.current_idx)
            .map(|m| {
                doc.content_index.md_to_main_ranges(
                    std::slice::from_ref(&m.md_range),
                    doc.markdown,
                    doc.content_offset,
                )
            })
            .unwrap_or_default();

        crate::frame::HighlightSpec {
            target_ranges,
            active_ranges,
        }
    }
}

pub(super) fn handle(
    action: InlineSearchAction,
    is: &mut InlineSearchState,
    doc: &DocumentQuery,
    max_scroll: u32,
) -> Vec<Effect> {
    match action {
        InlineSearchAction::Type(c) => {
            is.query.push(c);
            vec![Effect::RedrawInlineSearch]
        }
        InlineSearchAction::Backspace => {
            if is.query.is_empty() {
                return vec![
                    Effect::ScrollAnchor(is.pre_search_y),
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                ];
            }
            is.query.pop();
            vec![Effect::RedrawInlineSearch]
        }
        InlineSearchAction::Confirm => {
            if is.query.is_empty() {
                return vec![Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)];
            }
            let (matches, _valid) = grep_markdown(doc, &is.query);
            is.matches = matches;
            if is.matches.is_empty() {
                return vec![
                    Effect::Flash("Pattern not found".into()),
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                ];
            }
            is.current_idx = match is.direction {
                SearchDirection::Forward => first_match_from(is, doc, max_scroll),
                SearchDirection::Backward => last_match_before(is, doc, max_scroll),
            };
            let last = LastSearch::from_inline_search(is, doc);
            let vl_idx = is.matches[is.current_idx].visual_line_idx;
            let y = visual_line_offset(doc.visual_lines, max_scroll, (vl_idx + 1) as u32);
            let flash = format!("match {}/{}", is.current_idx + 1, is.matches.len());
            vec![
                Effect::SetLastSearch(last),
                Effect::InvalidateOverlays,
                Effect::ScrollAnchor(y),
                Effect::Flash(flash),
                Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
            ]
        }
        InlineSearchAction::Cancel => {
            vec![
                Effect::ScrollAnchor(is.pre_search_y),
                Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
            ]
        }
    }
}

/// Index of the first match at or after `pre_search_y`.
fn first_match_from(is: &InlineSearchState, doc: &DocumentQuery, max_scroll: u32) -> usize {
    for (i, m) in is.matches.iter().enumerate() {
        let line_num = (m.visual_line_idx + 1) as u32;
        let y = visual_line_offset(doc.visual_lines, max_scroll, line_num);
        if y >= is.pre_search_y {
            return i;
        }
    }
    0
}

/// Index of the last match at or before `pre_search_y`.
fn last_match_before(is: &InlineSearchState, doc: &DocumentQuery, max_scroll: u32) -> usize {
    let mut result = is.matches.len().saturating_sub(1);
    for (i, m) in is.matches.iter().enumerate().rev() {
        let line_num = (m.visual_line_idx + 1) as u32;
        let y = visual_line_offset(doc.visual_lines, max_scroll, line_num);
        if y <= is.pre_search_y {
            result = i;
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::query::test_helpers::*;
    use super::*;

    #[test]
    fn type_updates_query_without_searching() {
        let md = "hello world\nfoo bar";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0, SearchDirection::Forward);
        let effects = handle(InlineSearchAction::Type('f'), &mut is, &doc, 1000);
        assert_eq!(is.query, "f");
        assert!(is.matches.is_empty());
        assert!(!effects.iter().any(|e| matches!(e, Effect::ScrollAnchor(_))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RedrawInlineSearch))
        );
    }

    #[test]
    fn backspace_pops_query() {
        let md = "hello world";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0, SearchDirection::Forward);
        is.query = "he".into();
        let effects = handle(InlineSearchAction::Backspace, &mut is, &doc, 1000);
        assert_eq!(is.query, "h");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RedrawInlineSearch))
        );
    }

    #[test]
    fn backspace_on_empty_cancels() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(42, SearchDirection::Forward);
        let effects = handle(InlineSearchAction::Backspace, &mut is, &doc, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ScrollAnchor(42)))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }

    #[test]
    fn confirm_searches_and_sets_last_search() {
        let md = "hello world";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0, SearchDirection::Forward);
        // Type does not search — only updates query
        handle(InlineSearchAction::Type('h'), &mut is, &doc, 1000);
        assert!(is.matches.is_empty());
        // Confirm triggers the actual search
        let effects = handle(InlineSearchAction::Confirm, &mut is, &doc, 1000);
        assert!(!is.matches.is_empty());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetLastSearch(_)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::ScrollAnchor(_))));
    }

    #[test]
    fn confirm_no_match_flashes() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0, SearchDirection::Forward);
        handle(InlineSearchAction::Type('z'), &mut is, &doc, 1000);
        let effects = handle(InlineSearchAction::Confirm, &mut is, &doc, 1000);
        assert!(effects.iter().any(|e| matches!(e, Effect::Flash(_))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }

    #[test]
    fn confirm_empty_returns_to_normal() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0, SearchDirection::Forward);
        let effects = handle(InlineSearchAction::Confirm, &mut is, &doc, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }

    #[test]
    fn cancel_restores_scroll() {
        let md = "hello";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(99, SearchDirection::Forward);
        let effects = handle(InlineSearchAction::Cancel, &mut is, &doc, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ScrollAnchor(99)))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }
}
