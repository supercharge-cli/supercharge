//! `/context`: show detailed context usage (instant, not queued).
//! `/context <tokens>|reset`: override the current model's context window (saved to config).

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Show context usage breakdown, or set/reset the current model's context window.
pub struct ContextCommand;

impl SlashCommand for ContextCommand {
    slash_meta! {
        name: "context",
        description: "View context usage, or set the model's context window (/context 1m | 256k | reset)",
        usage: "/context [<tokens>|reset]",
        takes_args: true,
        session_scoped: true,
        arg_placeholder: "[<tokens>|reset]",
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".to_string());
        }
        let args = args.trim();
        if args.is_empty() {
            return CommandResult::Action(Action::ShowContextInfo);
        }
        match supercharge_shell::agent::parse_context_window(args) {
            Ok(value) => CommandResult::Action(Action::SetContextWindowOverride { value }),
            Err(error) => CommandResult::Error(error),
        }
    }
}
