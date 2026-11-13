# Usage Stats and Privacy

Supercharge AI sends a small amount of **anonymous** usage data so the team can see how
many installs are active, which versions and platforms are in use, and which features
people rely on. This page lists exactly what is sent and how to turn it off.

## What is collected

Every event carries:

| Field | Example | Notes |
| --- | --- | --- |
| Anonymous install id | `6f1f0b7e-…` | Random UUID created on first run and stored in `~/.supercharge/usage-stats.json`. Not derived from your machine, account, or name. |
| `source` / `app` | `cli` | Distinguishes the CLI from the desktop app. |
| `app_version` | `1.4.23` | |
| `platform`, `os`, `arch` | `terminal`, `linux`, `x86_64` | |
| `install_method` | `npm`, `gh-release` | Best-effort guess. |
| `run_mode` | `tui`, `headless`, `acp`, `leader`, `command` | How the process was started. |

Events are sent without a person profile (`$process_person_profile: false`), and the CLI
does not put an IP address in the event (`$ip: null`). The stats service may still store
the IP address the request arrives from, but no location is derived from it
(`$geoip_disable: true`).

Events:

| Event | When | Extra fields |
| --- | --- | --- |
| `app_opened` | A `supercharge` invocation starts | — |
| `command_run` | A subcommand runs | Subcommand name only (`models`, `login`, `agent stdio`, …). Never argument values. |
| `session_started` | A chat session is created, loaded, or resumed | `session_kind`: `new` / `load` / `resume` |
| `model_selected` | A session picks its model | `model` id; `provider`: built-in provider id (the same ids `/setup` shows), `local` for a loopback server, or `custom` for any other endpoint; `api_style`: `chat_completions`, `responses`, or `messages`. Endpoint URLs are never sent. |
| `acp_session_started` | An editor starts a session over ACP (`supercharge agent stdio`) | `session_kind` |
| `error` | The CLI exits with an error or panics | `kind`: `startup_failure`, `fatal_error`, or `panic`; `code`: exit code. No messages or stack traces. |

## What is never collected

Prompts, code, file contents, file names or paths, model output, API keys or tokens,
usernames, hostnames, email addresses, environment variables, or repository details.
Events are built from fixed labels, so free text cannot slip in.

Analytics are sent to PostHog (US region) in small batches from a background thread with
short timeouts. If the network is slow or offline, events are dropped silently; the CLI
never waits on them during startup and waits at most about one second at exit.

## Turning it off

Any one of these disables usage stats:

```bash
supercharge telemetry off          # saved in ~/.supercharge/usage-stats.json
export SUPERCHARGE_TELEMETRY=0     # per shell / per process
export DO_NOT_TRACK=1              # the cross-tool convention
```

Or in `~/.supercharge/config.toml` (admins can pin this in `requirements.toml`):

```toml
[telemetry]
usage_stats = false
```

Check the current state with `supercharge telemetry status`; turn it back on with
`supercharge telemetry on`.

Usage stats are also off automatically in CI environments (`CI`, `GITHUB_ACTIONS`, and
similar variables), in development builds, and in tests. `SUPERCHARGE_TELEMETRY=1`
forces them on in those cases.

The first time stats are enabled in an interactive terminal, Supercharge prints a
one-line notice explaining the above.
