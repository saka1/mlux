//! Scenario tests for the search → highlight-rect pipeline.

use super::test_harness::TestHarness;

#[test]
fn search_no_match_produces_no_rects() {
    let mut h = TestHarness::new("# Title\n\nfoo bar baz\n", 80, 24);
    h.feed_keys("/zzzzz\n");
    // Confirm with zero matches does not set last_search
    assert!(h.viewport.last_search.is_none());
    assert!(h.highlight_rects(0).is_empty());
}

#[test]
fn cancel_search_no_highlights() {
    let mut h = TestHarness::new("# Title\n\nfoo bar baz\n", 80, 24);
    h.feed_keys("/foo\x1b");
    // Cancel (Esc) does not set last_search
    assert!(h.viewport.last_search.is_none());
    assert!(h.highlight_rects(0).is_empty());
}

#[test]
fn search_produces_highlight_rects() {
    let mut h = TestHarness::new("# Title\n\nfoo bar baz\n\nfoo end\n", 80, 24);
    h.feed_keys("/foo\n");
    assert!(h.viewport.last_search.is_some());
    let rects = h.highlight_rects(0);
    assert!(
        !rects.is_empty(),
        "search for 'foo' should produce highlight rects"
    );
}

#[test]
fn search_marks_active_match() {
    let mut h = TestHarness::new("# Title\n\nfoo bar baz\n\nfoo end\n", 80, 24);
    h.feed_keys("/foo\n");
    let rects = h.highlight_rects(0);
    let active_count = rects.iter().filter(|r| r.is_active).count();
    let inactive_count = rects.iter().filter(|r| !r.is_active).count();
    // 2 matches (one per line), first is active
    assert!(active_count > 0, "current match should have active rects");
    assert!(
        inactive_count > 0,
        "other matches should have inactive rects"
    );
}

#[test]
fn navigate_changes_active_match() {
    let mut h = TestHarness::new("# Title\n\nfoo bar baz\n\nfoo end\n", 80, 24);
    h.feed_keys("/foo\n");
    let before: Vec<(u32, u32)> = h
        .highlight_rects(0)
        .iter()
        .filter(|r| r.is_active)
        .map(|r| (r.x_px, r.y_px))
        .collect();

    h.feed_keys("n");

    let after: Vec<(u32, u32)> = h
        .highlight_rects(0)
        .iter()
        .filter(|r| r.is_active)
        .map(|r| (r.x_px, r.y_px))
        .collect();

    assert_ne!(before, after, "n should change which match is active");
}

#[test]
fn search_match_count_aligns_with_rects() {
    let md = "# Title\n\nabc def\n\nghi abc\n\nabc jkl\n";
    let mut h = TestHarness::new(md, 80, 24);
    h.feed_keys("/abc\n");
    let ls = h.viewport.last_search.as_ref().unwrap();
    assert_eq!(ls.matches.len(), 3, "should find 3 lines matching 'abc'");
    let rects = h.highlight_rects(0);
    // Each match produces at least 1 rect (contiguous glyph run)
    assert!(
        rects.len() >= 3,
        "expected at least 3 highlight rects for 3 matches, got {}",
        rects.len()
    );
}
