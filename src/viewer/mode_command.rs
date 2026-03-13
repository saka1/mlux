//! Command mode handler (`:` prompt).

use super::effect::ExitReason;
use super::input::CommandAction;
use super::{Effect, ViewerMode};

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
                vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty]
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
                    vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty]
                }
                "reload" | "rel" => vec![Effect::Exit(ExitReason::ConfigReload)],
                "q" | "quit" => vec![Effect::Exit(ExitReason::Quit)],
                "back" | "b" => vec![Effect::Exit(ExitReason::GoBack)],
                "open" => vec![Effect::EnterUrlPickerAll],
                _ => vec![
                    Effect::SetMode(ViewerMode::Normal),
                    Effect::Flash(format!("Unknown command: {cmd}")),
                    Effect::MarkDirty,
                ],
            }
        }
        CommandAction::Cancel => vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty],
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
                .any(|e| matches!(e, Effect::SetMode(ViewerMode::Normal)))
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
        assert!(matches!(effects[0], Effect::Exit(ExitReason::ConfigReload)));
    }

    #[test]
    fn execute_quit() {
        let mut cs = CommandState { input: "q".into() };
        let effects = handle(CommandAction::Execute, &mut cs);
        assert!(matches!(effects[0], Effect::Exit(ExitReason::Quit)));
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
