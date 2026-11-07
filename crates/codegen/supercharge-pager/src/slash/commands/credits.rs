//! `/credits` opens a popup and loads the selected Mistral balance in the background.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub const LOADING_MARK: &str = "\u{0}mistral-credits-loading:";
pub const MODAL_TITLE: &str = "Mistral credits";

pub fn loading_body(frame: usize) -> String {
    let frames = crate::glyphs::braille_spinner_frames();
    let glyph = frames
        .get(frame % frames.len().max(1))
        .copied()
        .unwrap_or("");
    format!("{LOADING_MARK}{frame}\n\n{glyph}  Reading your Mistral balance…")
}

pub fn is_loading(content: &str) -> bool {
    content.starts_with(LOADING_MARK)
}

pub fn spin_loading_modal(modal: &mut crate::views::modal::ActiveModal) -> bool {
    let crate::views::modal::ActiveModal::DocViewer {
        title,
        content,
        cached_lines,
        ..
    } = modal
    else {
        return false;
    };
    if title != MODAL_TITLE || !is_loading(content) {
        return false;
    }
    let frame = content
        .strip_prefix(LOADING_MARK)
        .and_then(|rest| rest.lines().next())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    *content = loading_body(frame.wrapping_add(1));
    *cached_lines = None;
    true
}

pub fn finish_loading_modal(modal: &mut crate::views::modal::ActiveModal, body: String) -> bool {
    let crate::views::modal::ActiveModal::DocViewer {
        title,
        content,
        cached_lines,
        scroll,
        ..
    } = modal
    else {
        return false;
    };
    if title != MODAL_TITLE || !is_loading(content) {
        return false;
    }
    *content = body;
    *cached_lines = None;
    *scroll = 0;
    true
}

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
        CommandResult::Action(Action::ShowMistralCredits)
    }
}
