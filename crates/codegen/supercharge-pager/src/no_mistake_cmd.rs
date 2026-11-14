//! `supercharge no-mistake` — run the validation gate without the TUI.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use supercharge_shell::no_mistake::{GateRequest, OutputKind, exit_code, parse_tokens, run_in_dir};

/// Validate committed work through the Supercharge review, test, and lint gate.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "no-mistake",
    about = "Validate committed work through the Supercharge review, test, and lint gate",
    long_about = "Runs the Supercharge validation gate in an isolated git worktree so your checkout is left untouched.\n\n\
Bare `supercharge no-mistake` validates already-committed work on a feature branch.\n\
`supercharge no-mistake <task>` records that task as intent, then validates the committed work.\n\n\
Ask-user findings never auto-resolve, including when --yes is passed. A non-interactive run fails closed if a human decision is required."
)]
pub struct NoMistakeArgs {
    /// Comma-separated steps to skip (intent, isolate, rebase, review, test, document, lint, push, pr, ci).
    #[arg(long, value_name = "STEPS")]
    pub skip: Option<String>,
    /// Stop after local checks; do not push, open a pull request, or watch CI.
    #[arg(long)]
    pub no_publish: bool,
    /// Include publish steps when they are available.
    #[arg(long)]
    pub publish: bool,
    /// Integration branch to rebase onto and, later, to open a pull request against.
    #[arg(long, value_name = "BRANCH")]
    pub base_branch: Option<String>,
    /// Apply auto-fixable findings without prompting. Ask-user findings still stop the gate.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
    /// Final report format: plain or json.
    #[arg(long = "output-format", value_name = "FMT", default_value = "plain")]
    pub output_format: String,
    /// Read intent from this file instead of a task argument.
    #[arg(long = "intent-file", value_name = "PATH")]
    pub intent_file: Option<PathBuf>,
    /// Optional task used as intent. Must come after flags.
    #[arg(trailing_var_arg = true, allow_hyphen_values = false)]
    pub task: Vec<String>,
}

impl NoMistakeArgs {
    pub fn to_request(&self) -> Result<GateRequest, String> {
        let mut tokens = Vec::new();
        if let Some(skip) = &self.skip {
            tokens.push("--skip".to_string());
            tokens.push(skip.clone());
        }
        if self.no_publish {
            tokens.push("--no-publish".to_string());
        }
        if self.publish {
            tokens.push("--publish".to_string());
        }
        if let Some(branch) = &self.base_branch {
            tokens.push("--base-branch".to_string());
            tokens.push(branch.clone());
        }
        if self.yes {
            tokens.push("--yes".to_string());
        }
        tokens.push("--output-format".to_string());
        tokens.push(self.output_format.clone());
        if let Some(path) = &self.intent_file {
            tokens.push("--intent-file".to_string());
            tokens.push(path.display().to_string());
        }
        tokens.extend(self.task.iter().cloned());
        parse_tokens(&tokens).map_err(|e| e.to_string())
    }
}

/// Run the subcommand. Non-success gate outcomes exit the process with a stable code
/// after the report is printed (0 pass, 1 fail, 2 needs review, 130 cancelled).
pub fn run(args: NoMistakeArgs) -> Result<()> {
    let request = args.to_request().map_err(|e| anyhow::anyhow!("{e}"))?;
    let output = request.output;
    let cwd = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("Could not read the current directory: {e}"))?;
    match run_in_dir(&cwd, request) {
        Ok(report) => {
            let text = report.render(output);
            if output == OutputKind::Json {
                println!("{text}");
            } else {
                print!("{text}");
            }
            let code = exit_code(report.outcome);
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clap_maps_flags_onto_the_shared_parser() {
        let args = NoMistakeArgs::parse_from([
            "no-mistake",
            "--skip",
            "lint,document",
            "--no-publish",
            "--base-branch",
            "develop",
            "-y",
            "fix the typo",
        ]);
        let request = args.to_request().expect("request");
        assert!(request.no_publish);
        assert!(request.yes);
        assert_eq!(request.base_branch.as_deref(), Some("develop"));
        assert_eq!(request.task, "fix the typo");
        assert_eq!(request.skip.len(), 2);
    }

    #[test]
    fn conflicting_publish_flags_are_rejected() {
        let args = NoMistakeArgs::parse_from(["no-mistake", "--publish", "--no-publish"]);
        let err = args.to_request().expect_err("conflict");
        assert!(err.contains("--publish"));
    }
}
