//! Normal mode handler: scrolling, jumping, yanking, mode transitions.

use log::debug;

use super::input::Action;
use super::mode_command::CommandState;
use super::mode_search::{LastSearch, SearchState};
use super::mode_url::{UrlPickerEntry, UrlPickerState, collect_all_url_entries};
use super::state::{ExitReason, ViewState, visual_line_offset};
use super::{Effect, ViewerMode};
use crate::tile::{VisualLine, extract_urls, yank_exact, yank_lines};

pub(super) struct NormalCtx<'a> {
    pub state: &'a ViewState,
    pub visual_lines: &'a [VisualLine],
    pub max_scroll: u32,
    pub scroll_step: u32,
    pub half_page: u32,
    pub markdown: &'a str,
    pub last_search: &'a mut Option<LastSearch>,
}

pub(super) fn handle(action: Action, ctx: &mut NormalCtx) -> Vec<Effect> {
    match action {
        Action::Quit => vec![Effect::Exit(ExitReason::Quit)],

        Action::CancelInput => vec![Effect::RedrawStatusBar],

        Action::Digit => vec![Effect::RedrawStatusBar],

        Action::ScrollDown(count) => {
            let y = (ctx.state.y_offset + count * ctx.scroll_step).min(ctx.max_scroll);
            debug!(
                "scroll down: y_offset {} → {} (count={count}, step={}, max={})",
                ctx.state.y_offset, y, ctx.scroll_step, ctx.max_scroll
            );
            vec![Effect::ScrollTo(y)]
        }
        Action::ScrollUp(count) => {
            let y = ctx.state.y_offset.saturating_sub(count * ctx.scroll_step);
            debug!(
                "scroll up: y_offset {} → {} (count={count}, step={}, max={})",
                ctx.state.y_offset, y, ctx.scroll_step, ctx.max_scroll
            );
            vec![Effect::ScrollTo(y)]
        }
        Action::HalfPageDown(count) => {
            let y = (ctx.state.y_offset + count * ctx.half_page).min(ctx.max_scroll);
            debug!(
                "scroll half-down: y_offset {} → {} (count={count}, step={}, max={})",
                ctx.state.y_offset, y, ctx.half_page, ctx.max_scroll
            );
            vec![Effect::ScrollTo(y)]
        }
        Action::HalfPageUp(count) => {
            let y = ctx.state.y_offset.saturating_sub(count * ctx.half_page);
            debug!(
                "scroll half-up: y_offset {} → {} (count={count}, step={}, max={})",
                ctx.state.y_offset, y, ctx.half_page, ctx.max_scroll
            );
            vec![Effect::ScrollTo(y)]
        }

        Action::JumpToTop => {
            debug!("scroll top: y_offset {} → 0", ctx.state.y_offset);
            vec![Effect::ScrollTo(0)]
        }
        Action::JumpToBottom => {
            debug!(
                "scroll bottom: y_offset {} → {} (max={})",
                ctx.state.y_offset, ctx.max_scroll, ctx.max_scroll
            );
            vec![Effect::ScrollTo(ctx.max_scroll)]
        }
        Action::JumpToLine(n) => {
            let y = visual_line_offset(ctx.visual_lines, ctx.max_scroll, n);
            debug!("jump to line {n}: y_offset {} → {}", ctx.state.y_offset, y);
            vec![Effect::ScrollTo(y)]
        }

        Action::EnterSearch => {
            let ss = SearchState::new();
            vec![
                Effect::DeletePlacements,
                Effect::SetMode(ViewerMode::Search(ss)),
            ]
        }

        Action::EnterCommand => {
            let cs = CommandState {
                input: String::new(),
            };
            vec![Effect::SetMode(ViewerMode::Command(cs))]
        }

        Action::SearchNextMatch => navigate_search(ctx, SearchDirection::Next),
        Action::SearchPrevMatch => navigate_search(ctx, SearchDirection::Prev),

        Action::YankExactPrompt => {
            vec![
                Effect::Flash("Type Ny to yank line N".into()),
                Effect::RedrawStatusBar,
            ]
        }
        Action::YankExact(n) => yank_and_flash(ctx, n, yank_exact, |n, lc| {
            format!("Yanked L{n} ({lc} line{})", if lc > 1 { "s" } else { "" })
        }),

        Action::YankBlockPrompt => {
            vec![
                Effect::Flash("Type NY to yank block N".into()),
                Effect::RedrawStatusBar,
            ]
        }
        Action::YankBlock(n) => yank_and_flash(
            ctx,
            n,
            |md, vls, idx| yank_lines(md, vls, idx, idx),
            |n, lc| format!("Yanked L{n} block ({lc} lines)"),
        ),

        Action::OpenUrlPrompt => {
            vec![
                Effect::Flash("Type No to open URL on line N".into()),
                Effect::RedrawStatusBar,
            ]
        }
        Action::OpenUrl(n) => open_url(ctx, n),

        Action::EnterUrlPicker => {
            let entries = collect_all_url_entries(ctx.markdown, ctx.visual_lines);
            if entries.is_empty() {
                vec![
                    Effect::Flash("No URLs in document".into()),
                    Effect::RedrawStatusBar,
                ]
            } else {
                vec![
                    Effect::DeletePlacements,
                    Effect::SetMode(ViewerMode::UrlPicker(UrlPickerState::new(entries))),
                ]
            }
        }
    }
}

enum SearchDirection {
    Next,
    Prev,
}

fn navigate_search(ctx: &mut NormalCtx, direction: SearchDirection) -> Vec<Effect> {
    let Some(ls) = ctx.last_search.as_mut() else {
        return vec![
            Effect::Flash("No search results".into()),
            Effect::RedrawStatusBar,
        ];
    };
    match direction {
        SearchDirection::Next => ls.advance_next(),
        SearchDirection::Prev => ls.advance_prev(),
    }
    let Some(vl_idx) = ls.current_visual_line_idx() else {
        return vec![];
    };
    let line_num = (vl_idx + 1) as u32;
    let y = visual_line_offset(ctx.visual_lines, ctx.max_scroll, line_num);
    let flash = format!("match {}/{}", ls.current_idx + 1, ls.matches.len());
    vec![Effect::ScrollTo(y), Effect::Flash(flash)]
}

/// Shared yank logic: bounds check, extract text, build effects.
///
/// `extract` performs the actual text extraction (yank_exact or yank_lines).
/// `format_msg` builds the success flash message from (line_num, line_count).
fn yank_and_flash(
    ctx: &NormalCtx,
    line_num: u32,
    extract: impl FnOnce(&str, &[VisualLine], usize) -> String,
    format_msg: impl FnOnce(u32, usize) -> String,
) -> Vec<Effect> {
    let vl_idx = (line_num as usize).saturating_sub(1);
    if vl_idx >= ctx.visual_lines.len() {
        return vec![
            Effect::Flash(format!(
                "Line {line_num} out of range (max {})",
                ctx.visual_lines.len()
            )),
            Effect::RedrawStatusBar,
        ];
    }
    let text = extract(ctx.markdown, ctx.visual_lines, vl_idx);
    if text.is_empty() {
        return vec![
            Effect::Flash(format!("L{line_num}: no source mapping")),
            Effect::RedrawStatusBar,
        ];
    }
    let line_count = text.lines().count();
    debug!("yank L{line_num}: {} bytes, {line_count} lines", text.len());
    vec![
        Effect::Yank(text),
        Effect::Flash(format_msg(line_num, line_count)),
        Effect::RedrawStatusBar,
    ]
}

/// Open URL(s) found on the given visual line.
///
/// If exactly one URL, opens it directly. If multiple, enters URL picker
/// with only the URLs from that line.
fn open_url(ctx: &NormalCtx, line_num: u32) -> Vec<Effect> {
    let vl_idx = (line_num as usize).saturating_sub(1);
    if vl_idx >= ctx.visual_lines.len() {
        return vec![
            Effect::Flash(format!(
                "Line {line_num} out of range (max {})",
                ctx.visual_lines.len()
            )),
            Effect::RedrawStatusBar,
        ];
    }
    if ctx.visual_lines[vl_idx].md_line_range.is_none() {
        return vec![
            Effect::Flash(format!("L{line_num}: no source mapping")),
            Effect::RedrawStatusBar,
        ];
    }
    let urls = extract_urls(ctx.markdown, ctx.visual_lines, vl_idx);
    if urls.is_empty() {
        return vec![
            Effect::Flash(format!("L{line_num}: no URL found")),
            Effect::RedrawStatusBar,
        ];
    }
    if urls.len() == 1 {
        debug!("open_url L{line_num}: {}", urls[0].url);
        vec![
            Effect::OpenUrl(urls[0].url.clone()),
            Effect::Flash(format!("Opening {}", urls[0].url)),
            Effect::RedrawStatusBar,
        ]
    } else {
        debug!("open_url L{line_num}: {} URLs, entering picker", urls.len());
        let entries: Vec<UrlPickerEntry> = urls
            .into_iter()
            .map(|u| UrlPickerEntry {
                url: u.url,
                text: u.text,
                visual_line: line_num as usize,
            })
            .collect();
        vec![
            Effect::DeletePlacements,
            Effect::SetMode(ViewerMode::UrlPicker(UrlPickerState::new(entries))),
        ]
    }
}
