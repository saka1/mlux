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
    CancelInput,
    /// A digit was accumulated; caller should redraw status bar.
    Digit,
}

/// Map a key event to an `Action`, consuming/updating the accumulator as needed.
///
/// Returns `None` for unknown keys (caller should reset accumulator).
pub(super) fn map_key_event(key: KeyEvent, acc: &mut InputAccumulator) -> Option<Action> {
    let KeyEvent { code, modifiers, .. } = key;

    match (code, modifiers) {
        // 終了 (always immediate)
        (KeyCode::Char('q'), _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            Some(Action::Quit)
        }

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
        (KeyCode::Char('g'), _) => {
            match acc.take() {
                None => Some(Action::JumpToTop),
                Some(n) => Some(Action::JumpToLine(n)),
            }
        }
        // 末尾 / ジャンプ
        (KeyCode::Char('G'), _) => {
            match acc.take() {
                None => Some(Action::JumpToBottom),
                Some(n) => Some(Action::JumpToLine(n)),
            }
        }

        // 精密ヤンク (y)
        (KeyCode::Char('y'), _) => {
            match acc.take() {
                None => Some(Action::YankExactPrompt),
                Some(n) => Some(Action::YankExact(n)),
            }
        }

        // ブロックヤンク (Y)
        (KeyCode::Char('Y'), _) => {
            match acc.take() {
                None => Some(Action::YankBlockPrompt),
                Some(n) => Some(Action::YankBlock(n)),
            }
        }

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
}
