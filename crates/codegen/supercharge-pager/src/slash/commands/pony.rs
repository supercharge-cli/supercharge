//! `/save-tokens` — toggle built-in Ponytail (YAGNI) mode to cut token use.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct PonyCommand;

impl SlashCommand for PonyCommand {
    fn name(&self) -> &str {
        "save-tokens"
    }

    fn aliases(&self) -> &[&str] {
        &["pony", "ponytail"]
    }

    fn description(&self) -> &str {
        "Shorter YAGNI replies (saves tokens): /save-tokens on|off"
    }

    fn usage(&self) -> &str {
        "/save-tokens [on|off]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("on|off")
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(vec![
            ArgItem {
                display: "on".into(),
                match_text: "on".into(),
                insert_text: "on".into(),
                description: "Enable token-saving YAGNI mode".into(),
            },
            ArgItem {
                display: "off".into(),
                match_text: "off".into(),
                insert_text: "off".into(),
                description: "Disable token-saving mode".into(),
            },
        ])
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim().to_ascii_lowercase();
        let currently = crate::ponytail::is_enabled();
        let new = match trimmed.as_str() {
            "" | "toggle" => !currently,
            "on" | "true" | "1" | "enable" => true,
            "off" | "false" | "0" | "disable" => false,
            other => {
                return CommandResult::Error(format!(
                    "Usage: /save-tokens on|off (got `{other}`). Currently {}.",
                    if currently { "on" } else { "off" }
                ));
            }
        };
        CommandResult::Action(Action::SetPonytail(new))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;

    static BUNDLE: BundleState = BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn ctx<'a>(models: &'a ModelState) -> CommandExecCtx<'a> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        }
    }

    #[test]
    fn on_and_off_dispatch() {
        let models = ModelState::default();
        let mut c = ctx(&models);
        assert!(matches!(
            PonyCommand.run(&mut c, "on"),
            CommandResult::Action(Action::SetPonytail(true))
        ));
        assert!(matches!(
            PonyCommand.run(&mut c, "off"),
            CommandResult::Action(Action::SetPonytail(false))
        ));
    }

    #[test]
    fn bad_arg_errors() {
        let models = ModelState::default();
        let mut c = ctx(&models);
        assert!(matches!(PonyCommand.run(&mut c, "maybe"), CommandResult::Error(_)));
    }
}
