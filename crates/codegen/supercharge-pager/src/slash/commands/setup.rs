//! `/setup` -- open Supercharge AI's provider setup wizard.
//!
//! The wizard asks for an OpenAI-compatible base URL and API key, fetches
//! `/models` from that endpoint, and lets the user pick the default model.
use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};
/// Open the Supercharge AI provider setup wizard.
pub struct SetupCommand;
impl SlashCommand for SetupCommand {
    fn name(&self) -> &str {
        "setup"
    }
    fn description(&self) -> &str {
        "Configure provider base URL, API key, and choose a model from that API"
    }
    fn usage(&self) -> &str {
        "/setup"
    }
    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenSetup)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    static DEFAULT_BUNDLE_STATE: crate::app::bundle::BundleState =
        crate::app::bundle::BundleState {
            has_cache: false,
            version: String::new(),
            personas: Vec::new(),
            roles: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            persona_details: Vec::new(),
            role_details: Vec::new(),
        };
    #[test]
    fn opens_settings_modal() {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            SetupCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenSetup)
        ));
    }
    #[test]
    fn setup_accepts_extra_arguments_without_bypassing_modal_validation() {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Minimal,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            SetupCommand.run(&mut ctx, "provider pollinations"),
            CommandResult::Action(Action::OpenSetup)
        ));
    }
}
