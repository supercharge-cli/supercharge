//! `/retry`: re-send the most recent prompt.
//!
//! Right after an interrupted turn (Esc stop gesture rewinds it), this issues the same
//! prompt again instead of retyping it. Skips slash commands and bash-mode echoes so the
//! retry is always a genuine agent prompt.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Re-issue the last prompt of the session.
pub struct RetryCommand;

impl SlashCommand for RetryCommand {
    slash_meta! {
        name: "retry",
        description: "Re-send the previous prompt",
        usage: "/retry",
        session_scoped: true,
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::RetryLastPrompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    #[test]
    fn run_dispatches_retry_last_prompt() {
        let cmd = RetryCommand;
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            cmd.run(&mut ctx, ""),
            CommandResult::Action(Action::RetryLastPrompt)
        ));
    }
}
