//! Command mode handler (`:` prompt).

use std::io;

use super::input::CommandAction;
use super::state::{ExitReason, Layout};
use super::terminal;
use super::{Effect, ViewerMode};

/// Mutable state for command mode (`:` prompt).
pub(super) struct CommandState {
    pub input: String,
}

pub(super) fn handle(
    action: CommandAction,
    cs: &mut CommandState,
    layout: &Layout,
) -> io::Result<Vec<Effect>> {
    match action {
        CommandAction::Type(c) => {
            cs.input.push(c);
            terminal::draw_command_bar(layout, &cs.input)?;
            Ok(vec![])
        }
        CommandAction::Backspace => {
            if cs.input.is_empty() {
                // Empty input + Backspace → cancel (vim behavior)
                Ok(vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty])
            } else {
                cs.input.pop();
                terminal::draw_command_bar(layout, &cs.input)?;
                Ok(vec![])
            }
        }
        CommandAction::Execute => {
            let cmd = cs.input.trim().to_string();
            match cmd.as_str() {
                "" => {
                    // Empty command → just return to normal
                    Ok(vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty])
                }
                "reload" | "rel" => Ok(vec![Effect::Exit(ExitReason::ConfigReload)]),
                "q" | "quit" => Ok(vec![Effect::Exit(ExitReason::Quit)]),
                _ => Ok(vec![
                    Effect::SetMode(ViewerMode::Normal),
                    Effect::Flash(format!("Unknown command: {cmd}")),
                    Effect::MarkDirty,
                ]),
            }
        }
        CommandAction::Cancel => Ok(vec![Effect::SetMode(ViewerMode::Normal), Effect::MarkDirty]),
    }
}
