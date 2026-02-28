//! Input processing layer: key mapping and numeric prefix accumulator.
//!
//! Pure logic, no I/O. All functions are deterministic and testable.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const MAX_LINE_NUM: u32 = 999_999;

/// Accumulated numeric prefix for vim/less-style commands.
///
/// Users type digits then a command character: `56g` jumps to line 56,
/// `10j` scrolls 10 steps down, `56y` yanks line 56.
pub(super) struct InputAccumulator {
    count: Option<u32>,
}

impl InputAccumulator {
    pub(super) fn new() -> Self {
        Self { count: None }
    }

    /// Feed a digit character ('0'..='9'). Returns false if overflow would occur.
    fn push_digit(&mut self, d: u32) -> bool {
        let current = self.count.unwrap_or(0);
        let new = current.saturating_mul(10).saturating_add(d);
        if new > MAX_LINE_NUM {
            return false; // ignore further digits
        }
        self.count = Some(new);
        true
    }

    /// Take the accumulated count, resetting to None.
    fn take(&mut self) -> Option<u32> {
        self.count.take()
    }

    /// Peek at the current accumulated count without consuming it.
    pub(super) fn peek(&self) -> Option<u32> {
        self.count
    }

    pub(super) fn reset(&mut self) {
        self.count = None;
    }

    pub(super) fn is_active(&self) -> bool {
        self.count.is_some()
    }
}

/// Actions produced by key input processing.
pub(super) enum Action {
    Quit,
    ScrollDown(u32),
    ScrollUp(u32),
    HalfPageDown(u32),
    HalfPageUp(u32),
    JumpToTop,
    JumpToBottom,
    JumpToLine(u32),
    YankExact(u32),
    YankExactPrompt,
    YankBlock(u32),
    YankBlockPrompt,
    OpenUrl(u32),
    OpenUrlPrompt,
    EnterSearch,
    EnterCommand,
    SearchNextMatch,
    SearchPrevMatch,
    CancelInput,
    /// A digit was accumulated; caller should redraw status bar.
    Digit,
}

/// Map a key event to an `Action`, consuming/updating the accumulator as needed.
///
/// Returns `None` for unknown keys (caller should reset accumulator).
pub(super) fn map_key_event(key: KeyEvent, acc: &mut InputAccumulator) -> Option<Action> {
    let KeyEvent {
        code, modifiers, ..
    } = key;

    match (code, modifiers) {
        // 終了 (always immediate)
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),

        // Esc: cancel pending input
        (KeyCode::Esc, _) => {
            acc.reset();
            Some(Action::CancelInput)
        }

        // Digits: accumulate
        (KeyCode::Char(c @ '0'..='9'), KeyModifiers::NONE) => {
            let d = c as u32 - '0' as u32;
            acc.push_digit(d);
            Some(Action::Digit)
        }

        // 下スクロール
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
            let count = acc.take().unwrap_or(1);
            Some(Action::ScrollDown(count))
        }
        // 上スクロール
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
            let count = acc.take().unwrap_or(1);
            Some(Action::ScrollUp(count))
        }
        // 半画面下
        (KeyCode::Char('d'), _) => {
            let count = acc.take().unwrap_or(1);
            Some(Action::HalfPageDown(count))
        }
        // 半画面上
        (KeyCode::Char('u'), _) => {
            let count = acc.take().unwrap_or(1);
            Some(Action::HalfPageUp(count))
        }
        // 先頭 / ジャンプ
        (KeyCode::Char('g'), _) => match acc.take() {
            None => Some(Action::JumpToTop),
            Some(n) => Some(Action::JumpToLine(n)),
        },
        // 末尾 / ジャンプ
        (KeyCode::Char('G'), _) => match acc.take() {
            None => Some(Action::JumpToBottom),
            Some(n) => Some(Action::JumpToLine(n)),
        },

        // 精密ヤンク (y)
        (KeyCode::Char('y'), _) => match acc.take() {
            None => Some(Action::YankExactPrompt),
            Some(n) => Some(Action::YankExact(n)),
        },

        // ブロックヤンク (Y)
        (KeyCode::Char('Y'), _) => match acc.take() {
            None => Some(Action::YankBlockPrompt),
            Some(n) => Some(Action::YankBlock(n)),
        },

        // URL を開く (o)
        (KeyCode::Char('o'), _) => match acc.take() {
            None => Some(Action::OpenUrlPrompt),
            Some(n) => Some(Action::OpenUrl(n)),
        },

        // 検索
        (KeyCode::Char('/'), _) => {
            acc.reset();
            Some(Action::EnterSearch)
        }
        // コマンドモード
        (KeyCode::Char(':'), _) => {
            acc.reset();
            Some(Action::EnterCommand)
        }
        // 次のマッチへジャンプ
        (KeyCode::Char('n'), KeyModifiers::NONE) => {
            acc.reset();
            Some(Action::SearchNextMatch)
        }
        // 前のマッチへジャンプ
        (KeyCode::Char('N'), KeyModifiers::SHIFT) => {
            acc.reset();
            Some(Action::SearchPrevMatch)
        }

        _ => None,
    }
}

/// Actions specific to search mode.
pub(super) enum SearchAction {
    Type(char),
    Backspace,
    SelectNext,
    SelectPrev,
    Confirm,
    Cancel,
}

/// Actions specific to command mode (`:` prompt).
pub(super) enum CommandAction {
    Type(char),
    Backspace,
    Execute,
    Cancel,
}

/// Map a key event to a command-mode action.
pub(super) fn map_command_key(key: KeyEvent) -> Option<CommandAction> {
    let KeyEvent {
        code, modifiers, ..
    } = key;

    match (code, modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            Some(CommandAction::Cancel)
        }
        (KeyCode::Enter, _) => Some(CommandAction::Execute),
        (KeyCode::Backspace, _) => Some(CommandAction::Backspace),
        (KeyCode::Char(c), _) => Some(CommandAction::Type(c)),
        _ => None,
    }
}

/// Map a key event to a search-mode action.
pub(super) fn map_search_key(key: KeyEvent) -> Option<SearchAction> {
    let KeyEvent {
        code, modifiers, ..
    } = key;

    match (code, modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            Some(SearchAction::Cancel)
        }
        (KeyCode::Enter, _) => Some(SearchAction::Confirm),
        (KeyCode::Backspace, _) => Some(SearchAction::Backspace),
        (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
            Some(SearchAction::SelectNext)
        }
        (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
            Some(SearchAction::SelectPrev)
        }
        (KeyCode::Char(c), _) => Some(SearchAction::Type(c)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn simple_key(code: KeyCode) -> KeyEvent {
        key(code, KeyModifiers::NONE)
    }

    #[test]
    fn test_5j_scroll_down() {
        let mut acc = InputAccumulator::new();
        // Type '5'
        let a = map_key_event(simple_key(KeyCode::Char('5')), &mut acc);
        assert!(matches!(a, Some(Action::Digit)));
        // Type 'j'
        let a = map_key_event(simple_key(KeyCode::Char('j')), &mut acc);
        assert!(matches!(a, Some(Action::ScrollDown(5))));
    }

    #[test]
    fn test_g_without_prefix_jumps_top() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char('g')), &mut acc);
        assert!(matches!(a, Some(Action::JumpToTop)));
    }

    #[test]
    fn test_56g_jumps_to_line() {
        let mut acc = InputAccumulator::new();
        map_key_event(simple_key(KeyCode::Char('5')), &mut acc);
        map_key_event(simple_key(KeyCode::Char('6')), &mut acc);
        let a = map_key_event(simple_key(KeyCode::Char('g')), &mut acc);
        assert!(matches!(a, Some(Action::JumpToLine(56))));
    }

    #[test]
    fn test_q_quits() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char('q')), &mut acc);
        assert!(matches!(a, Some(Action::Quit)));
    }

    #[test]
    fn test_ctrl_c_quits() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut acc);
        assert!(matches!(a, Some(Action::Quit)));
    }

    #[test]
    fn test_esc_cancels_input() {
        let mut acc = InputAccumulator::new();
        map_key_event(simple_key(KeyCode::Char('5')), &mut acc);
        assert!(acc.is_active());
        let a = map_key_event(simple_key(KeyCode::Esc), &mut acc);
        assert!(matches!(a, Some(Action::CancelInput)));
        assert!(!acc.is_active());
    }

    #[test]
    fn test_unknown_key_returns_none() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char('x')), &mut acc);
        assert!(a.is_none());
    }

    #[test]
    fn test_yank_with_prefix() {
        let mut acc = InputAccumulator::new();
        map_key_event(simple_key(KeyCode::Char('3')), &mut acc);
        let a = map_key_event(simple_key(KeyCode::Char('y')), &mut acc);
        assert!(matches!(a, Some(Action::YankExact(3))));
    }

    #[test]
    fn test_yank_without_prefix() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char('y')), &mut acc);
        assert!(matches!(a, Some(Action::YankExactPrompt)));
    }

    #[test]
    fn test_big_g_bottom() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(key(KeyCode::Char('G'), KeyModifiers::SHIFT), &mut acc);
        assert!(matches!(a, Some(Action::JumpToBottom)));
    }

    // --- Open URL ---

    #[test]
    fn test_o_open_url_prompt() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char('o')), &mut acc);
        assert!(matches!(a, Some(Action::OpenUrlPrompt)));
    }

    #[test]
    fn test_5o_open_url() {
        let mut acc = InputAccumulator::new();
        map_key_event(simple_key(KeyCode::Char('5')), &mut acc);
        let a = map_key_event(simple_key(KeyCode::Char('o')), &mut acc);
        assert!(matches!(a, Some(Action::OpenUrl(5))));
    }

    // --- Search: normal mode entry ---

    #[test]
    fn test_slash_enters_search() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char('/')), &mut acc);
        assert!(matches!(a, Some(Action::EnterSearch)));
    }

    #[test]
    fn test_slash_resets_accumulator() {
        let mut acc = InputAccumulator::new();
        map_key_event(simple_key(KeyCode::Char('5')), &mut acc);
        assert!(acc.is_active());
        map_key_event(simple_key(KeyCode::Char('/')), &mut acc);
        assert!(!acc.is_active());
    }

    #[test]
    fn test_n_search_next() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char('n')), &mut acc);
        assert!(matches!(a, Some(Action::SearchNextMatch)));
    }

    #[test]
    fn test_big_n_search_prev() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(key(KeyCode::Char('N'), KeyModifiers::SHIFT), &mut acc);
        assert!(matches!(a, Some(Action::SearchPrevMatch)));
    }

    // --- Search mode: map_search_key ---

    #[test]
    fn test_search_type_char() {
        let a = map_search_key(simple_key(KeyCode::Char('a')));
        assert!(matches!(a, Some(SearchAction::Type('a'))));
    }

    #[test]
    fn test_search_backspace() {
        let a = map_search_key(simple_key(KeyCode::Backspace));
        assert!(matches!(a, Some(SearchAction::Backspace)));
    }

    #[test]
    fn test_search_select_next_j() {
        let a = map_search_key(simple_key(KeyCode::Char('j')));
        assert!(matches!(a, Some(SearchAction::SelectNext)));
    }

    #[test]
    fn test_search_select_next_down() {
        let a = map_search_key(simple_key(KeyCode::Down));
        assert!(matches!(a, Some(SearchAction::SelectNext)));
    }

    #[test]
    fn test_search_select_prev_k() {
        let a = map_search_key(simple_key(KeyCode::Char('k')));
        assert!(matches!(a, Some(SearchAction::SelectPrev)));
    }

    #[test]
    fn test_search_select_prev_up() {
        let a = map_search_key(simple_key(KeyCode::Up));
        assert!(matches!(a, Some(SearchAction::SelectPrev)));
    }

    #[test]
    fn test_search_confirm() {
        let a = map_search_key(simple_key(KeyCode::Enter));
        assert!(matches!(a, Some(SearchAction::Confirm)));
    }

    #[test]
    fn test_search_cancel_esc() {
        let a = map_search_key(simple_key(KeyCode::Esc));
        assert!(matches!(a, Some(SearchAction::Cancel)));
    }

    #[test]
    fn test_search_cancel_ctrl_c() {
        let a = map_search_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(a, Some(SearchAction::Cancel)));
    }

    #[test]
    fn test_search_unknown_returns_none() {
        let a = map_search_key(simple_key(KeyCode::Tab));
        assert!(a.is_none());
    }

    // --- Command mode: map_command_key ---

    #[test]
    fn test_colon_enters_command() {
        let mut acc = InputAccumulator::new();
        let a = map_key_event(simple_key(KeyCode::Char(':')), &mut acc);
        assert!(matches!(a, Some(Action::EnterCommand)));
    }

    #[test]
    fn test_command_type_char() {
        let a = map_command_key(simple_key(KeyCode::Char('r')));
        assert!(matches!(a, Some(CommandAction::Type('r'))));
    }

    #[test]
    fn test_command_backspace() {
        let a = map_command_key(simple_key(KeyCode::Backspace));
        assert!(matches!(a, Some(CommandAction::Backspace)));
    }

    #[test]
    fn test_command_execute() {
        let a = map_command_key(simple_key(KeyCode::Enter));
        assert!(matches!(a, Some(CommandAction::Execute)));
    }

    #[test]
    fn test_command_cancel_esc() {
        let a = map_command_key(simple_key(KeyCode::Esc));
        assert!(matches!(a, Some(CommandAction::Cancel)));
    }

    #[test]
    fn test_command_cancel_ctrl_c() {
        let a = map_command_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(a, Some(CommandAction::Cancel)));
    }
}
