//! `/imodels` — pick or type an image-generation model.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct IModelsCommand;

impl SlashCommand for IModelsCommand {
    fn name(&self) -> &str {
        "imodels"
    }

    fn aliases(&self) -> &[&str] {
        &["imagine-model", "image-model", "imodel"]
    }

    fn description(&self) -> &str {
        "Switch the image-generation model (or open /isetup)"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/imodels [model-id]"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let model = args.trim();
        if model.is_empty() {
            return CommandResult::Action(Action::OpenImageSetup);
        }
        match supercharge_shell::agent::image_provider::persist_image_model(model) {
            Ok(()) => CommandResult::Message(format!(
                "Image model set to `{model}`. Open a new session so /imagine picks it up."
            )),
            Err(error) => CommandResult::Error(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    #[test]
    fn empty_opens_isetup() {
        let models = ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        assert!(matches!(
            IModelsCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenImageSetup)
        ));
    }
}
