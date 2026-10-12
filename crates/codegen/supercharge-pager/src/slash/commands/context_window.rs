use std::num::NonZeroU64;

use crate::app::actions::Action;
use crate::slash::command::{
    AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand, slash_meta,
};

pub struct ContextWindowCommand;

impl SlashCommand for ContextWindowCommand {
    slash_meta! {
        name: "context-window",
        description: "Set the context window for the current model",
        usage: "/context-window <size>",
        takes_args: true,
        args_required: true,
        session_scoped: true,
        arg_placeholder: "<size>",
    }

    fn visible(&self, ctx: &AppCtx) -> bool {
        ctx.models.context_window_options().len() > 1
    }

    fn suggest_args(&self, ctx: &AppCtx, _query: &str) -> Option<Vec<ArgItem>> {
        let options = ctx.models.context_window_options();
        (options.len() > 1).then(|| window_arg_items(&options, ctx.models.get_context_window(), ""))
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let Some(model_id) = ctx.models.current.clone() else {
            return CommandResult::Error("No active model".into());
        };
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session yet".into());
        }
        let options = ctx.models.context_window_options();
        if options.len() < 2 {
            return CommandResult::Error("current model has no selectable context windows".into());
        }
        match supported_window(args.trim(), &options) {
            Some(window) => CommandResult::Action(Action::SwitchContextWindow {
                model_id,
                effort: ctx.models.reasoning_effort,
                window,
            }),
            None => CommandResult::Error(format!(
                "unknown context window '{}'; use one of: {}",
                args.trim(),
                options
                    .iter()
                    .map(|&w| format_window(w))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

pub(crate) fn supported_window(token: &str, options: &[u64]) -> Option<NonZeroU64> {
    let token = token.to_ascii_lowercase();
    let parsed = if let Some(n) = token.strip_suffix('k') {
        n.parse::<u64>().ok()?.checked_mul(1_000)?
    } else if let Some(n) = token.strip_suffix('m') {
        n.parse::<u64>().ok()?.checked_mul(1_000_000)?
    } else {
        token.parse::<u64>().ok()?
    };
    options
        .contains(&parsed)
        .then_some(parsed)
        .and_then(NonZeroU64::new)
}

pub(crate) fn format_window(window: u64) -> String {
    if window >= 1_000_000 && window.is_multiple_of(1_000_000) {
        format!("{}m", window / 1_000_000)
    } else if window >= 1_000 && window.is_multiple_of(1_000) {
        format!("{}k", window / 1_000)
    } else {
        window.to_string()
    }
}

pub(crate) fn window_arg_items(options: &[u64], active: Option<u64>, prefix: &str) -> Vec<ArgItem> {
    options
        .iter()
        .map(|&window| {
            let label = format_window(window);
            let insert_text = format!("{prefix}{label}");
            ArgItem {
                display: if active == Some(window) {
                    format!("{label} (active)")
                } else {
                    label
                },
                match_text: insert_text.clone(),
                insert_text,
                description: format!("{window} tokens"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hides_without_multiple_advertised_windows() {
        let models = crate::acp::model_state::ModelState::default();
        let ctx = AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        assert!(!ContextWindowCommand.visible(&ctx));
        assert!(ContextWindowCommand.suggest_args(&ctx, "").is_none());
        let id = agent_client_protocol::ModelId::new(std::sync::Arc::from("single"));
        let mut single = crate::acp::model_state::ModelState::default();
        single.available.insert(
            id.clone(),
            agent_client_protocol::ModelInfo::new(id.clone(), "Single").meta(
                serde_json::json!({"contextWindows": [256_000]})
                    .as_object()
                    .cloned(),
            ),
        );
        single.current = Some(id);
        let ctx = AppCtx {
            models: &single,
            ..ctx
        };
        assert!(!ContextWindowCommand.visible(&ctx));
    }

    #[test]
    fn supported_sizes_only() {
        let options = [256_000, 500_000];
        assert_eq!(
            supported_window("500K", &options).map(NonZeroU64::get),
            Some(500_000)
        );
        assert_eq!(
            supported_window("500000", &options).map(NonZeroU64::get),
            Some(500_000)
        );
        for invalid in ["", "0", "1m", "-1", "18446744073709551615m"] {
            assert!(supported_window(invalid, &options).is_none(), "{invalid}");
        }
    }
}
