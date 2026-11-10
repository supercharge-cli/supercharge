use agent_client_protocol as acp;

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct CodecovCommand;

/// Build the structured prompt the agent executes for `/codecov`.
/// `scope` is the raw args text; empty args mean the whole workspace.
///
/// Everything runs locally: no uploads, no hosted services, and capped
/// parallelism so the checks stay light on CPU and RAM.
fn codecov_instruction(scope: &str) -> String {
    let scope_line = if scope.trim().is_empty() {
        "Scope: the whole workspace.".to_string()
    } else {
        format!("Scope: {}.", scope.trim())
    };
    format!(
        "Run LOCAL code-quality checks for this project (coverage + code duplication), report the\n\
         results, and ask before fixing anything. Everything must run on this machine: never upload\n\
         reports or source anywhere, never call hosted analysis services, and never send data over\n\
         the network.\n\
         \n\
         1. Coverage: detect this project's canonical local coverage tooling (for example\n\
         cargo-llvm-cov or cargo-tarpaulin for Rust, pytest-cov for Python, `go test -cover` for Go,\n\
         jest or vitest with --coverage for JavaScript). Prefer whatever the repo already configures;\n\
         if nothing is set up, report that and propose a tool instead of installing it.\n\
         {scope_line}\n\
         Keep resource usage low: cap build and test parallelism (e.g. `-j 2` and\n\
         `--test-threads 2`) and reuse incremental artifacts instead of clean rebuilds. If the run\n\
         fails, report the failure and stop.\n\
         2. Duplication: run a local duplicate-code check using the lightest available method (a\n\
         repo-configured dup checker if present; otherwise a small one-off token-similarity scan you\n\
         write and run locally). Report duplicated blocks of 6+ lines with `file:start-end` locations\n\
         and their similarity, largest first. Do not install heavyweight analyzers.\n\
         3. Report: overall coverage percentage, per-file or per-module coverage for the weakest\n\
         areas, the highest-impact uncovered functions and branches, then the duplication hotspots.\n\
         4. Then stop and ask me whether to fix the gaps by writing tests and deduplicating code.\n\
         Do not write or modify any code until I confirm."
    )
}

impl SlashCommand for CodecovCommand {
    slash_meta! {
        name: "codecov",
        description: "Run local coverage and duplication checks, report, and ask whether to fix",
        usage: "/codecov [scope]",
        takes_args: true,
        args_required: false,
        session_scoped: true,
        arg_placeholder: "scope (e.g. crate, package, or path)",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        CommandResult::InjectSkill {
            display_text: if args.trim().is_empty() {
                "/codecov".to_string()
            } else {
                format!("/codecov {}", args.trim())
            },
            prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(
                codecov_instruction(args),
            ))],
            display_as_skill: false,
            scheduled_task_preview: None,
        }
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

    fn prompt_text(result: &CommandResult) -> String {
        match result {
            CommandResult::InjectSkill { prompt_blocks, .. } => prompt_blocks
                .iter()
                .filter_map(|block| match block {
                    acp::ContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }

    #[test]
    fn bare_run_injects_whole_workspace_instruction() {
        let result = run("");
        match &result {
            CommandResult::InjectSkill { display_text, .. } => {
                assert_eq!(display_text, "/codecov");
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
        let text = prompt_text(&result);
        assert!(text.contains("LOCAL code-quality checks"));
        assert!(text.contains("never upload"));
        assert!(text.contains("Scope: the whole workspace."));
        assert!(text.contains("Duplication:"));
        assert!(text.contains("duplicated blocks of 6+ lines"));
        assert!(text.contains("cap build and test parallelism"));
        assert!(text.contains("ask me whether to fix"));
        assert!(text.contains("Do not write or modify any code until I confirm."));
    }

    #[test]
    fn args_become_the_scope() {
        let result = run("supercharge-pager");
        match &result {
            CommandResult::InjectSkill { display_text, .. } => {
                assert_eq!(display_text, "/codecov supercharge-pager");
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
        let text = prompt_text(&result);
        assert!(text.contains("Scope: supercharge-pager."));
    }

    #[test]
    fn args_are_trimmed_in_display_text() {
        match run("  src/lib.rs  ") {
            CommandResult::InjectSkill { display_text, .. } => {
                assert_eq!(display_text, "/codecov src/lib.rs");
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }
}
