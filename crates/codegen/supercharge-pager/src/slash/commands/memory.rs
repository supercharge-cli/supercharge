//! `/memory` opens the browser; existing shell subcommands remain available.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct MemoryCommand;

impl SlashCommand for MemoryCommand {
    slash_meta! {
        name: "memory",
        aliases: ["mem"],
        description: "Browse and manage saved memories",
        usage: "/memory [on|off|status]",
        takes_args: true,
        session_scoped: true,
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        match args.trim() {
            "" => CommandResult::Action(Action::OpenMemoryModal),
            "on" | "off" | "status" => {
                CommandResult::PassThrough(format!("/memory {}", args.trim()))
            }
            _ => CommandResult::Error("Usage: /memory [on|off|status]".into()),
        }
    }
}
