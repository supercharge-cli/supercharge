//! `/codecov`: run the local coverage + duplication engine and report, or
//! hand the confirmed gaps to the agent with `/codecov fix`.
//!
//! The check phase is fully native (`supercharge-codecov`): no AI, no network,
//! capped parallelism. Only the fix phase sends anything to the model.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// What `/codecov` should do; `scope` restricts the analysis to a path prefix.
#[derive(Debug)]
pub enum CodecovRequest {
    Report { scope: String },
    Fix { scope: String },
}

impl CodecovRequest {
    pub fn scope(&self) -> &str {
        match self {
            Self::Report { scope } | Self::Fix { scope } => scope,
        }
    }

    pub fn is_fix(&self) -> bool {
        matches!(self, Self::Fix { .. })
    }
}

pub struct CodecovCommand;

impl SlashCommand for CodecovCommand {
    slash_meta! {
        name: "codecov",
        description: "Run local coverage and duplication checks, report, and ask whether to fix",
        usage: "/codecov [fix] [scope]",
        takes_args: true,
        args_required: false,
        session_scoped: true,
        arg_placeholder: "[fix] [scope]",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let args = args.trim();
        let (fix, scope) = match args.strip_prefix("fix") {
            Some(rest) => (true, rest.trim()),
            None => (false, args),
        };
        CommandResult::Codecov(CodecovRequest {
            fix,
            scope: scope.to_string(),
        })
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
        CodecovCommand.run(&mut context, args)
    }

    #[test]
    fn bare_run_requests_a_report_for_the_whole_workspace() {
        match run("") {
            CommandResult::Codecov(request) => {
                assert!(!request.is_fix());
                assert_eq!(request.scope(), "");
            }
            other => panic!("expected Codecov, got {other:?}"),
        }
    }

    #[test]
    fn fix_prefix_sets_the_fix_flag_and_keeps_the_scope() {
        match run("fix src/") {
            CommandResult::Codecov(request) => {
                assert!(request.is_fix());
                assert_eq!(request.scope(), "src/");
            }
            other => panic!("expected Codecov, got {other:?}"),
        }
    }

    #[test]
    fn scope_only_args_stay_a_report() {
        match run("crates/codegen/supercharge-pager") {
            CommandResult::Codecov(request) => {
                assert!(!request.is_fix());
                assert_eq!(request.scope(), "crates/codegen/supercharge-pager");
            }
            other => panic!("expected Codecov, got {other:?}"),
        }
    }
}
