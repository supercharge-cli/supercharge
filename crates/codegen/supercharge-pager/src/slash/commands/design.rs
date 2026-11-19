//! `/design` forwards to the shell builtin that toggles design mode or starts a design task.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Design mode for UI work.
pub struct DesignCommand;

impl SlashCommand for DesignCommand {
    slash_meta! {
        name: "design",
        description: "Design mode for UI work: on, off, status, or describe a component to build and verify",
        usage: "/design on|off|status|<what to build>",
        takes_args: true,
        args_required: false,
        session_scoped: true,
        arg_placeholder: "on|off|status|<what to build>",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let args = args.trim();
        let text = if args.is_empty() {
            "/design".to_string()
        } else {
            format!("/design {args}")
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
        DesignCommand.run(&mut context, args)
    }

    #[test]
    fn bare_run_queues_the_status_command() {
        match run("  ") {
            CommandResult::QueueCommand(text) => assert_eq!(text, "/design"),
            other => panic!("expected QueueCommand, got {other:?}"),
        }
    }

    #[test]
    fn forwards_toggles_and_tasks() {
        match run("on") {
            CommandResult::QueueCommand(text) => assert_eq!(text, "/design on"),
            other => panic!("expected QueueCommand, got {other:?}"),
        }
        match run("a combobox for picking a country ") {
            CommandResult::QueueCommand(text) => {
                assert_eq!(text, "/design a combobox for picking a country")
            }
            other => panic!("expected QueueCommand, got {other:?}"),
        }
    }
}
