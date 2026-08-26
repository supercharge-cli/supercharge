//! `/isetup` — image-generation provider wizard.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct ISetupCommand;

impl SlashCommand for ISetupCommand {
    fn name(&self) -> &str {
        "isetup"
    }

    fn aliases(&self) -> &[&str] {
        &["imagine-setup", "image-setup"]
    }

    fn description(&self) -> &str {
        "Configure image generation: endpoint, API key, format, and model"
    }

    fn usage(&self) -> &str {
        "/isetup"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenImageSetup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    #[test]
    fn opens_image_setup() {
        let models = ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        assert!(matches!(
            ISetupCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenImageSetup)
        ));
    }
}
