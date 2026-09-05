//! `/donate` — open the Supercharge / XibeBase support page.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub const DONATE_URL: &str = "https://ai.xibebase.in/";

pub struct DonateCommand;

impl SlashCommand for DonateCommand {
    fn name(&self) -> &str {
        "donate"
    }

    fn aliases(&self) -> &[&str] {
        &["support", "sponsor"]
    }

    fn description(&self) -> &str {
        "Open the Supercharge support page"
    }

    fn usage(&self) -> &str {
        "/donate"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenUrl(DONATE_URL.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::slash::command::CommandExecCtx;

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

    #[test]
    fn opens_donate_url() {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        };
        match DonateCommand.run(&mut ctx, "") {
            CommandResult::Action(Action::OpenUrl(url)) => assert_eq!(url, DONATE_URL),
            other => panic!("expected OpenUrl, got {other:?}"),
        }
    }
}
