//! Command mode handler (`:` prompt).

use super::Effect;
use super::effect::ExitReason;
use super::effect::{ScreenRestore, ViewerMode};
use super::keymap::CommandAction;
use super::mode_grep::GrepState;

/// Mutable state for command mode (`:` prompt).
pub(super) struct CommandState {
    pub input: String,
}

pub(super) fn handle(action: CommandAction, cs: &mut CommandState) -> Vec<Effect> {
    match action {
        CommandAction::Type(c) => {
            cs.input.push(c);
            vec![Effect::RedrawCommandBar]
        }
        CommandAction::Backspace => {
            if cs.input.is_empty() {
                // Empty input + Backspace → cancel (vim behavior)
                vec![
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                    Effect::MarkDirty,
                ]
            } else {
                cs.input.pop();
                vec![Effect::RedrawCommandBar]
            }
        }
        CommandAction::Execute => {
            let cmd = cs.input.trim().to_string();
            match cmd.as_str() {
                "" => {
                    // Empty command → just return to normal
                    vec![
                        Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                        Effect::MarkDirty,
                    ]
                }
                "reload" | "rel" => vec![Effect::Exit(ExitReason::Reload)],
                "q" | "quit" => vec![Effect::Exit(ExitReason::Quit)],
                "back" | "b" => vec![Effect::Exit(ExitReason::GoBack)],
                "open" => vec![Effect::EnterUrlPickerAll],
                "log" => vec![Effect::EnterLog],
                "watch" | "w" => vec![
                    Effect::ToggleWatch,
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                    Effect::MarkDirty,
                ],
                "noh" => vec![
                    Effect::HideHighlights,
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                ],
                "grep" | "g" => {
                    let gs = GrepState::new();
                    vec![
                        Effect::DeletePlacements,
                        Effect::SetMode(ViewerMode::Grep(gs)),
                    ]
                }
                _ => vec![
                    Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
                    Effect::Flash(format!("Unknown command: {cmd}")),
                    Effect::MarkDirty,
                ],
            }
        }
        CommandAction::Cancel => vec![
            Effect::ExitToNormal(ScreenRestore::StatusBarRefresh),
            Effect::MarkDirty,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_appends_and_redraws() {
        let mut cs = CommandState {
            input: String::new(),
        };
        let effects = handle(CommandAction::Type('r'), &mut cs);
        assert_eq!(cs.input, "r");
        assert!(matches!(effects[0], Effect::RedrawCommandBar));
    }

    #[test]
    fn backspace_empty_cancels() {
        let mut cs = CommandState {
            input: String::new(),
        };
        let effects = handle(CommandAction::Backspace, &mut cs);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }

    #[test]
    fn backspace_non_empty_pops_and_redraws() {
        let mut cs = CommandState { input: "re".into() };
        let effects = handle(CommandAction::Backspace, &mut cs);
        assert_eq!(cs.input, "r");
        assert!(matches!(effects[0], Effect::RedrawCommandBar));
    }

    #[test]
    fn execute_reload() {
        let mut cs = CommandState {
            input: "reload".into(),
        };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(matches!(effects[0], Effect::Exit(ExitReason::Reload)));
    }

    #[test]
    fn execute_quit() {
        let mut cs = CommandState { input: "q".into() };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(matches!(effects[0], Effect::Exit(ExitReason::Quit)));
    }

    #[test]
    fn execute_log() {
        let mut cs = CommandState {
            input: "log".into(),
        };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(effects.iter().any(|e| matches!(e, Effect::EnterLog)));
    }

    #[test]
    fn execute_grep() {
        let mut cs = CommandState {
            input: "grep".into(),
        };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::DeletePlacements))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Grep(_))))
        );
    }

    #[test]
    fn execute_g_alias() {
        let mut cs = CommandState { input: "g".into() };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Grep(_))))
        );
    }

    #[test]
    fn execute_watch() {
        let mut cs = CommandState {
            input: "watch".into(),
        };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(effects.iter().any(|e| matches!(e, Effect::ToggleWatch)));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkDirty)));
    }

    #[test]
    fn execute_w_alias() {
        let mut cs = CommandState { input: "w".into() };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(effects.iter().any(|e| matches!(e, Effect::ToggleWatch)));
    }

    #[test]
    fn execute_noh_hides_highlights() {
        let mut cs = CommandState {
            input: "noh".into(),
        };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(effects.iter().any(|e| matches!(e, Effect::HideHighlights)));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExitToNormal(ScreenRestore::StatusBarRefresh)))
        );
    }

    #[test]
    fn execute_unknown_flashes() {
        let mut cs = CommandState {
            input: "foobar".into(),
        };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Flash(msg) if msg.contains("Unknown command")))
        );
    }
}
