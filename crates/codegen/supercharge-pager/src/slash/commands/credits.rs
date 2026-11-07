//! `/credits` asks the session to read the selected provider's account balance.
//! Mistral Auth and Mistral API Key are handled in the shell so the TUI and ACP
//! use the same credential.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct CreditsCommand;

impl SlashCommand for CreditsCommand {
    slash_meta! {
        name: "credits",
        description: "Show the selected Mistral plan and usage balance",
        usage: "/credits",
        takes_args: false,
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if !args.trim().is_empty() {
            return CommandResult::Error("Usage: /credits".to_string());
        }
        CommandResult::QueueCommand("/credits".to_string())
    }
}
