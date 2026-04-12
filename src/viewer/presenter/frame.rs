//! Input type for a single redraw cycle.

use super::super::layout::{Layout, ScrollState};
use crate::frame::{DocumentMeta, HighlightSpec};

/// Everything the presenter needs to perform one full redraw.
pub(in super::super) struct PresentationFrame<'a> {
    pub meta: &'a DocumentMeta,
    pub layout: &'a Layout,
    pub scroll: &'a ScrollState,
    pub filename: &'a str,
    pub acc_peek: Option<u32>,
    pub flash: Option<&'a str>,
    pub search_spec: Option<&'a HighlightSpec>,
}
