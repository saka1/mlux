//! Inline (less-style) search mode: incremental search with status bar prompt.

use super::Effect;
use super::effect::ScreenRestore;
use super::keymap::InlineSearchAction;
use super::layout::visual_line_offset;
use super::mode_grep::{LastSearch, SearchMatch, grep_markdown};
use super::query::DocumentQuery;

/// State for inline search mode (`/` prompt in status bar).
pub(super) struct InlineSearchState {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub current_idx: usize,
    /// Scroll position before search started (restored on Esc).
    pub pre_search_y: u32,
}

impl InlineSearchState {
    pub(super) fn new(pre_search_y: u32) -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current_idx: 0,
            pre_search_y,
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
            search_and_jump(is, doc, max_scroll)
        }
        InlineSearchAction::Backspace => {
            if is.query.is_empty() {
                return vec![
                    Effect::InvalidateOverlays,
                    Effect::ScrollTo(is.pre_search_y),
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                ];
            }
            is.query.pop();
            if is.query.is_empty() {
                is.matches.clear();
                is.current_idx = 0;
                vec![
                    Effect::InvalidateOverlays,
                    Effect::ScrollTo(is.pre_search_y),
                    Effect::RedrawInlineSearch,
                ]
            } else {
                search_and_jump(is, doc, max_scroll)
            }
        }
        InlineSearchAction::Confirm => {
            if is.matches.is_empty() {
                return vec![
                    Effect::InvalidateOverlays,
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                ];
            }
            let last = LastSearch::from_inline_search(is, doc);
            let flash = format!("match {}/{}", is.current_idx + 1, is.matches.len());
            vec![
                Effect::SetLastSearch(last),
                Effect::Flash(flash),
                Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
            ]
        }
        InlineSearchAction::Cancel => {
            vec![
                Effect::InvalidateOverlays,
                Effect::ScrollTo(is.pre_search_y),
                Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
            ]
        }
    }
}

/// Run grep, update state, and return effects to jump to first match.
fn search_and_jump(
    is: &mut InlineSearchState,
    doc: &DocumentQuery,
    max_scroll: u32,
) -> Vec<Effect> {
    let (matches, _valid) = grep_markdown(doc, &is.query);
    is.matches = matches;
    is.current_idx = 0;

    let mut effects = vec![Effect::InvalidateOverlays];

    if let Some(m) = is.matches.first() {
        let line_num = (m.visual_line_idx + 1) as u32;
        let y = visual_line_offset(doc.visual_lines, max_scroll, line_num);
        effects.push(Effect::ScrollTo(y));
    }

    effects.push(Effect::RedrawInlineSearch);
    effects
}

#[cfg(test)]
mod tests {
    use super::super::query::test_helpers::*;
    use super::*;

    #[test]
    fn type_triggers_search_and_jump() {
        let md = "hello world\nfoo bar";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0);
        let effects = handle(InlineSearchAction::Type('f'), &mut is, &doc, 1000);
        assert_eq!(is.query, "f");
        assert!(!is.matches.is_empty());
        assert!(effects.iter().any(|e| matches!(e, Effect::ScrollTo(_))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::InvalidateOverlays))
        );
    }

    #[test]
    fn backspace_pops_and_researches() {
        let md = "hello world";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0);
        is.query = "he".into();
        let (matches, _) = grep_markdown(&doc, "he");
        is.matches = matches;
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
        let mut is = InlineSearchState::new(42);
        let effects = handle(InlineSearchAction::Backspace, &mut is, &doc, 1000);
        assert!(effects.iter().any(|e| matches!(e, Effect::ScrollTo(42))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }

    #[test]
    fn confirm_sets_last_search() {
        let md = "hello world";
        let vl = make_visual_lines(md);
        let ci = empty_ci();
        let doc = DocumentQuery::new(md, &vl, &ci, 0);
        let mut is = InlineSearchState::new(0);
        handle(InlineSearchAction::Type('h'), &mut is, &doc, 1000);
        let effects = handle(InlineSearchAction::Confirm, &mut is, &doc, 1000);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetLastSearch(_)))
        );
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
        let mut is = InlineSearchState::new(0);
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
        let mut is = InlineSearchState::new(99);
        let effects = handle(InlineSearchAction::Cancel, &mut is, &doc, 1000);
        assert!(effects.iter().any(|e| matches!(e, Effect::ScrollTo(99))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }
}
