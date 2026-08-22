# Supercharge AI

Supercharge AI is a Rust-first terminal agent for interactive coding, automation, browser workspaces, and long-running daemon deployments.

## Quick start

```bash
supercharge
```

Inside the TUI, run `/setup` to configure the AI provider, endpoint, model, and credentials. Provider configuration is stored under `~/.supercharge/` and is not read from global API-key environment variables.

Use `sc` as the short command alias:

```bash
sc
```

For headless or Agent Client Protocol integrations, use the command's `--help` output. The same `/setup`-managed provider configuration is used by interactive and noninteractive sessions.

## Core capabilities

Supercharge AI supports interactive TUI coding, model switching, persistent sessions, memory, project instructions, skills, plugins, MCP tools, browser workspaces, E2B environments, daemon operation, and messaging gateways.

Run `/help` inside the TUI to see available commands. Run `/setup` whenever the provider, endpoint, model, or credential needs to change.

## Local data

Sessions, configuration, memory, and extracted documentation live under `~/.supercharge/`. Existing installations under `~/.grok/` are migrated automatically when possible.

## Privacy

No provider credential is embedded in the binary. Enter credentials through `/setup` and keep the resulting local configuration protected with normal filesystem permissions.
