//! `/format` changes the saved provider's API format without repeating setup.
use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct FormatCommand;

impl SlashCommand for FormatCommand {
    fn name(&self) -> &str {
        "format"
    }
    fn description(&self) -> &str {
        "Change the saved API format: Chat Completions, Messages, or Responses"
    }
    fn usage(&self) -> &str {
        "/format"
    }
    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenFormatPicker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::slash::commands::tests::make_ctx;

    #[test]
    fn format_opens_picker_even_with_arguments() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        for args in ["", "responses"] {
            assert!(matches!(
                FormatCommand.run(&mut ctx, args),
                CommandResult::Action(Action::OpenFormatPicker)
            ));
        }
    }

    #[test]
    fn format_metadata_describes_all_choices() {
        assert_eq!(FormatCommand.name(), "format");
        assert_eq!(FormatCommand.usage(), "/format");
        for label in ["Chat Completions", "Messages", "Responses"] {
            assert!(FormatCommand.description().contains(label));
        }
    }
}
