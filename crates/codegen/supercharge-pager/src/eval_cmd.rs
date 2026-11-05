//! `supercharge eval` — run the agent against a task dataset and grade the
//! results, or run best-of-N on a single task.

use clap::Subcommand;

/// Arguments for the `eval` subcommand.
#[derive(Debug, clap::Args, Clone)]
pub struct EvalArgs {
    #[command(subcommand)]
    pub command: EvalCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum EvalCommand {
    /// Run the agent over a JSONL task dataset and grade the results
    Run {
        /// Dataset file, or directory of .jsonl files (required unless --preset)
        #[arg(long, value_name = "PATH", required_unless_present = "preset")]
        dataset: Option<std::path::PathBuf>,
        /// Built-in self-contained task set: smoke, rust, or web
        #[arg(long, value_name = "NAME", value_parser = ["smoke", "rust", "web"])]
        preset: Option<String>,
        /// Attempts per task (enables pass@k reporting)
        #[arg(long, default_value_t = 1, value_name = "K")]
        attempts: usize,
        /// Maximum concurrently running attempts
        #[arg(long, default_value_t = 4, value_name = "N")]
        concurrency: usize,
        /// Directory for run artifacts (run.json, grades.json, report.md)
        #[arg(long, default_value = ".supercharge-eval/runs", value_name = "DIR")]
        out_dir: std::path::PathBuf,
        /// Agent program invoked per attempt, run inside the task worktree
        #[arg(long, default_value = "supercharge", value_name = "PROGRAM")]
        agent: String,
        /// Agent arguments; {prompt} is replaced with the task instructions
        #[arg(long, default_value = "--single {prompt}", value_name = "ARGS")]
        agent_args: String,
    },
    /// Run best-of-N on a single task and write the winning patch
    BestOf {
        /// Task instructions given to the agent
        #[arg(long, value_name = "TEXT")]
        instructions: String,
        /// Repository the attempts run in
        #[arg(long, value_name = "PATH")]
        repo_path: std::path::PathBuf,
        /// Commit to base the attempts on
        #[arg(long, value_name = "COMMIT")]
        base_commit: Option<String>,
        /// Test command deciding success; zero exit status passes
        #[arg(long, value_name = "CMD")]
        test_cmd: String,
        /// Task wall-clock budget per attempt, in seconds
        #[arg(long, default_value_t = 300, value_name = "SECONDS")]
        timeout_seconds: u64,
        /// Number of attempts to race
        #[arg(long, default_value_t = 4, value_name = "N")]
        n: usize,
        /// Maximum concurrently running attempts
        #[arg(long, default_value_t = 4, value_name = "N")]
        concurrency: usize,
        /// Directory for run artifacts and the winning patch
        #[arg(long, default_value = ".supercharge-eval/runs", value_name = "DIR")]
        out_dir: std::path::PathBuf,
        /// Agent program invoked per attempt, run inside the task worktree
        #[arg(long, default_value = "supercharge", value_name = "PROGRAM")]
        agent: String,
        /// Agent arguments; {prompt} is replaced with the task instructions
        #[arg(long, default_value = "--single {prompt}", value_name = "ARGS")]
        agent_args: String,
    },
    /// Re-grade a saved run without invoking the agent
    Replay {
        /// Run directory (containing run.json) or the run.json file itself
        #[arg(long, value_name = "PATH")]
        run: std::path::PathBuf,
        /// Directory for the replayed grades and report
        #[arg(long, default_value = ".supercharge-eval/runs", value_name = "DIR")]
        out_dir: std::path::PathBuf,
    },
}

fn parse_agent(agent: &str, agent_args: &str) -> anyhow::Result<supercharge_eval::AgentCommand> {
    supercharge_eval::AgentCommand::parse(agent, agent_args)
        .ok_or_else(|| anyhow::anyhow!("could not parse --agent-args {agent_args:?}"))
}

pub async fn run(args: EvalArgs) -> anyhow::Result<()> {
    match args.command {
        EvalCommand::Run {
            dataset,
            preset,
            attempts,
            concurrency,
            out_dir,
            agent,
            agent_args,
        } => {
            let config = supercharge_eval::EvalConfig {
                agent: parse_agent(&agent, &agent_args)?,
                out_dir: out_dir.clone(),
                max_parallel: concurrency,
            };
            let summary = if let Some(name) = preset {
                if dataset.is_some() {
                    anyhow::bail!("pass either --dataset or --preset, not both");
                }
                let tasks = supercharge_eval::load_preset(&name, &out_dir)?;
                supercharge_eval::cmd::eval_run_tasks(&config, &tasks, attempts).await?
            } else {
                let dataset = dataset.expect("clap requires --dataset without --preset");
                supercharge_eval::cmd::eval_run(&config, &dataset, attempts).await?
            };
            println!(
                "pass@1 {:.3} over {} tasks (run {})",
                summary.pass_at_1, summary.tasks, summary.run_id
            );
            for (k, v) in summary.pass_at_k.iter().skip_while(|(k, _)| **k <= 1) {
                println!("pass@{k} {v:.3}");
            }
            println!(
                "report: {}/report.md",
                config.out_dir.join(&summary.run_id).display()
            );
        }
        EvalCommand::BestOf {
            instructions,
            repo_path,
            base_commit,
            test_cmd,
            timeout_seconds,
            n,
            concurrency,
            out_dir,
            agent,
            agent_args,
        } => {
            let task = supercharge_eval::Task {
                task_id: "best-of".to_string(),
                instructions,
                repo_path,
                base_commit,
                test_cmd: Some(test_cmd),
                timeout_seconds,
                env: Default::default(),
            };
            let config = supercharge_eval::EvalConfig {
                agent: parse_agent(&agent, &agent_args)?,
                out_dir,
                max_parallel: concurrency,
            };
            let (outcome, patch_path) =
                supercharge_eval::cmd::eval_best_of(&config, &task, n).await?;
            match outcome.winner {
                Some(index) => println!(
                    "winner: attempt {index} ({} of {} attempts passed)",
                    outcome.attempts.iter().filter(|a| a.grade.passed()).count(),
                    outcome.attempts.len(),
                ),
                None => println!("no attempt passed grading; nothing selected"),
            }
            println!(
                "run: {} (judged without pass: {})",
                config.out_dir.join(&outcome.run_id).display(),
                outcome.judged_without_pass,
            );
            println!("patch: {}", patch_path.display());
        }
        EvalCommand::Replay { run, out_dir } => {
            let summary = supercharge_eval::replay::replay(&run, &out_dir).await?;
            println!(
                "replayed {}: pass@1 {:.3} over {} tasks",
                summary.run_id, summary.pass_at_1, summary.tasks
            );
        }
    }
    Ok(())
}
