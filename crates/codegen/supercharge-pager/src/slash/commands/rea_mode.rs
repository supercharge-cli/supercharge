//! `/REA-mode` — toggle the built-in reverse-engineer-anything guide.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct ReaModeCommand;

impl SlashCommand for ReaModeCommand {
    fn name(&self) -> &str {
        "REA-mode"
    }

    fn aliases(&self) -> &[&str] {
        &["rea-mode", "rea"]
    }

    fn description(&self) -> &str {
        "Reverse-engineering guide in every turn: /REA-mode on|off"
    }

    fn usage(&self) -> &str {
        "/REA-mode [on|off]"
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
                description: "Add the reverse-engineer-anything guide".into(),
            },
            ArgItem {
                display: "off".into(),
                match_text: "off".into(),
                insert_text: "off".into(),
                description: "Remove the guide".into(),
            },
        ])
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let parsed = parse_arg(args);
        match parsed {
            Ok(None) => CommandResult::Message(crate::rea_mode::status_message(
                crate::rea_mode::is_enabled(),
            )),
            Ok(Some(on)) => CommandResult::Action(Action::SetReaMode(on)),
            Err(other) => CommandResult::Error(format!("Usage: /REA-mode on|off (got `{other}`).")),
        }
    }
}

/// `Ok(None)` shows the state; `Ok(Some(_))` sets it.
fn parse_arg(args: &str) -> Result<Option<bool>, String> {
    match args.trim().to_ascii_lowercase().as_str() {
        "" | "status" => Ok(None),
        "on" | "true" | "1" | "enable" => Ok(Some(true)),
        "off" | "false" | "0" | "disable" => Ok(Some(false)),
        _ => Err(args.trim().to_string()),
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
            ReaModeCommand.run(&mut c, "on"),
            CommandResult::Action(Action::SetReaMode(true))
        ));
        assert!(matches!(
            ReaModeCommand.run(&mut c, " OFF "),
            CommandResult::Action(Action::SetReaMode(false))
        ));
    }

    #[test]
    fn no_arg_shows_state_and_setup_hint() {
        crate::rea_mode::set_enabled(true);
        let models = ModelState::default();
        let mut c = ctx(&models);
        let CommandResult::Message(text) = ReaModeCommand.run(&mut c, "") else {
            panic!("expected a status message");
        };
        assert!(text.contains("REA mode is on"), "{text}");
        assert!(
            text.contains(supercharge_shared::rea::SETUP_COMMAND),
            "{text}"
        );
    }

    #[test]
    fn bad_arg_errors() {
        let models = ModelState::default();
        let mut c = ctx(&models);
        assert!(matches!(
            ReaModeCommand.run(&mut c, "maybe"),
            CommandResult::Error(_)
        ));
    }
}
