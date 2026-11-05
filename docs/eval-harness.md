# Eval harness and best-of-N runner

`supercharge eval` measures how well the agent solves tasks, and races N
attempts of one task to pick the best patch. It is fully offline: attempts run
the agent as a subprocess inside isolated `git worktree`s, and grading applies
each attempt's patch to a clean checkout and runs the task's test command.

## Dataset format

A dataset is a JSONL file (or a directory of `*.jsonl` files), one task per
line. Blank lines and `#` comments are ignored; `task_id`s must be unique.

```json
{"task_id": "fix-value", "instructions": "make value.txt contain `fixed`", "repo_path": "/path/to/repo", "base_commit": "abc123", "test_cmd": "test \"$(cat value.txt)\" = \"fixed\"", "timeout_seconds": 300, "env": {"KEY": "value"}}
```

| Field | Required | Meaning |
|---|---|---|
| `task_id` | yes | Unique identifier, used in artifact paths |
| `instructions` | yes | Prompt given to the agent |
| `repo_path` | yes | Local git repository; attempts run in worktrees of it |
| `base_commit` | no | Commit to base attempts on (default: HEAD) |
| `test_cmd` | no | Shell command deciding success; zero exit passes. Without it, a cleanly applied patch passes |
| `timeout_seconds` | no | Per-attempt wall-clock budget (default 300) |
| `env` | no | Extra environment variables for the agent and the test command |

## Running

```bash
# Grade every task once (pass@1)
supercharge eval run --dataset tasks.jsonl

# 5 attempts per task, 8 in parallel, custom agent
supercharge eval run --dataset tasks.jsonl --attempts 5 --concurrency 8 \
  --agent my-agent --agent-args "--headless {prompt}"

# Race 4 attempts of one task and write the winning patch
supercharge eval best-of --instructions "fix the failing test" \
  --repo-path . --test-cmd "cargo test" --n 4

# Re-grade a saved run with the current grader (no agent invocation)
supercharge eval replay --run .supercharge-eval/runs/<run_id>
```

Tip: when `--agent-args` starts with `-` (for example `-c "…"`), use the
`--agent-args='…'` form so clap does not parse it as a flag.

## Artifacts

Every run is persisted under the `--out-dir` (default
`.supercharge-eval/runs/<run_id>/`):

- `run.json` — per-task, per-attempt records: exit code, wall time, captured
  output tail, and the full patch (`git diff --cached --binary` after
  `git add -A`, so untracked and binary files are included).
- `grades.json` — grading classification per attempt:
  `pass | fail_apply | fail_tests | timeout | error`.
- `report.md` — human-readable summary: pass@1, unbiased pass@k (attempts ≥ k),
  median/p90/total wall time, and a per-attempt table.
- `winner.patch` — best-of-N only: the selected attempt's patch.

## Attempt lifecycle

1. `git worktree add --detach <out>/<run_id>/worktrees/<task>-<n>` at the base
   commit. Attempts are fully isolated from each other and from your checkout.
2. The agent command runs with its working directory set to the worktree and
   `SUPERCHARGE_EVAL_TASK_ID` / `SUPERCHARGE_EVAL_ATTEMPT` exported. A timed-out
   agent is killed (`kill_on_drop`), so no tokens burn after the budget expires.
3. The patch is captured, the worktree removed, and the attempt graded in a
   fresh checkout: apply the patch (`git apply`), run `test_cmd` under a
   600 s cap.

## Best-of-N selection

Default judge: the first attempt whose tests pass wins. When no attempt passes,
the attempt with the fewest changed lines is selected as the candidate patch so
a reviewer (human or model) sees the smallest diff. Selection is deterministic;
a model-scored judge can be added later behind the same `bestof::Judge` trait.

## Testing your own tasks without model credentials

The mock agent pattern used by the crate's own tests is a shell script:

```json
{"task_id": "t", "instructions": "x", "repo_path": "...", "test_cmd": "test ! -f marker"}
```

with `--agent sh --agent-args "touch marker"` — any program works, as long as
it edits files in its working directory.

## Built-in presets

For comparable scores across Supercharge versions, use a built-in preset.
Each preset materializes one or more temporary git repositories under
`<out-dir>/presets/<name>/` and grades against a fixed `test_cmd`, so the
task text and pass criteria do not drift with your local trees.

```bash
supercharge eval run --preset smoke
supercharge eval run --preset rust --attempts 3
supercharge eval run --preset web --concurrency 2
```

| Preset | Tasks | What passes |
|---|---|---|
| `smoke` | `smoke-fix-value` | `value.txt` equals `fixed` |
| `rust` | `rust-add-one` | `cargo test` in a mini crate (`add_one` returns `n + 1`) |
| `web` | `web-greeting` | `index.html` heading `#greeting` is exactly `Hello, Supercharge` |

`--preset` and `--dataset` are mutually exclusive. Preset repos are created
once per `out-dir` and reused on later runs (idempotent materialization).
