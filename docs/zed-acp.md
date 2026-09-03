# Supercharge in Zed (ACP)

Zed does **not** list Supercharge until you add it as an External Agent (or until the ACP Registry PR is merged). Auth is owned by Supercharge, not Zed.

## 1. Install the CLI

```bash
curl -fsSL https://raw.githubusercontent.com/iotserver24/supercharge-releases/main/scripts/install.sh | bash
```

Put `supercharge` on your `PATH`.

## 2. Register it in Zed (required for the picker)

```bash
supercharge acp setup --editor zed
```

This writes `~/.config/zed/settings.json` (`%APPDATA%\Zed\settings.json` on Windows):

```json
{
  "agent_servers": {
    "Supercharge": {
      "type": "custom",
      "command": "supercharge",
      "args": ["agent", "stdio"]
    }
  }
}
```

Zed reloads settings without a restart. Open the Agent Panel (`cmd-?` / `ctrl-?`) and start a **Supercharge** thread.

Preview the JSON without writing:

```bash
supercharge acp setup --editor zed --print
```

## Slash commands

Zed only shows `/` commands after `session/new` completes. Supercharge then sends `available_commands_update` (ACP spec). Type `/` in the Supercharge thread.

Useful commands:

| Command | What it does |
|---|---|
| `/help` or `/commands` | List commands |
| `/setup` | Show provider setup help |
| `/setup <provider> <api_key>` | Save a built-in provider (pollinations, openai, anthropic, openrouter, …) |
| `/setup <base_url> <api_key> [model]` | Save a custom OpenAI-compatible endpoint |
| `/session-info` | Session, model, context |
| `/compact` | Compress history |

After `/setup`, start a **new** Supercharge thread so the agent reloads `~/.supercharge/config.toml`.

## 3. Authenticate

| Situation | What to do |
|---|---|
| Provider already configured (`/setup` done in the TUI) | Zed uses `provider.api_key` or `cached_token` automatically. |
| First time | In the Supercharge thread, pick **Sign in** (browser OAuth) **or** **Configure provider in terminal** (`supercharge login` → `/setup` → quit). |
| Sign out | Zed logout calls ACP `logout`, or run `supercharge logout`. |

Do **not** point Zed at bare `supercharge` (that is the TUI). ACP is always:

```text
supercharge agent stdio
```

## 4. ACP Registry (global picker)

Files to submit after a release that includes these auth methods:

- [`docs/acp-registry/supercharge/agent.json`](acp-registry/supercharge/agent.json)
- [`docs/acp-registry/supercharge/icon.svg`](acp-registry/supercharge/icon.svg)

Open a PR against [agentclientprotocol/registry](https://github.com/agentclientprotocol/registry). Until that lands, step 2 is how users get Supercharge in the Zed agent list.
