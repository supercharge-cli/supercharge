//! `/no-mistake` forwards to the shell builtin that runs the validation gate.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Validate committed work through the Supercharge review, test, and lint gate.
pub struct NoMistakeCommand;

impl SlashCommand for NoMistakeCommand {
    slash_meta! {
        name: "no-mistake",
        description: "Validate committed work through the Supercharge review, test, and lint gate",
        usage: "/no-mistake [--skip=steps] [--no-publish] [--base-branch name] [<task>]",
        takes_args: true,
        args_required: false,
        session_scoped: true,
        arg_placeholder: "[--skip=steps] [--no-publish] [--base-branch name] [<task>]",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let text = if args.trim().is_empty() {
            "/no-mistake".to_string()
        } else {
            format!("/no-mistake {args}")
        };
        CommandResult::QueueCommand(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::slash::command::CommandExecCtx;

    fn run(args: &str) -> CommandResult {
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut context = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        };
        NoMistakeCommand.run(&mut context, args)
    }

    #[test]
    fn bare_run_queues_the_shell_command() {
        match run("") {
            CommandResult::QueueCommand(text) => assert_eq!(text, "/no-mistake"),
            other => panic!("expected QueueCommand, got {other:?}"),
        }
    }

    #[test]
    fn preserves_arguments() {
        match run("--skip=lint add a flag") {
            CommandResult::QueueCommand(text) => {
                assert_eq!(text, "/no-mistake --skip=lint add a flag")
            }
            other => panic!("expected QueueCommand, got {other:?}"),
        }
    }
}
