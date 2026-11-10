//! `/narrator`: toggle spoken turn summaries.
//!
//! When on, each completed turn speaks the opening of the final assistant message
//! through the system TTS program (`spd-say` on Linux, `say` on macOS). Off by
//! default; persisted to `[ui].narrator`.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Toggle spoken turn summaries.
pub struct NarratorCommand;

impl SlashCommand for NarratorCommand {
    slash_meta! {
        name: "narrator",
        description: "Toggle spoken turn summaries (system TTS)",
        usage: "/narrator",
        session_scoped: true,
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::ToggleNarrator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    #[test]
    fn run_dispatches_toggle_narrator() {
        let cmd = NarratorCommand;
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
            CommandResult::Action(Action::ToggleNarrator)
        ));
    }
}
