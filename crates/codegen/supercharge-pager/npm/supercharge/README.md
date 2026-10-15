# Supercharge

Bring Supercharge into your terminal. Fast, flicker-free CLI built for plans, subagents, and parallel work.

**[Homepage](https://superchargeai.in)** | **[Documentation](https://superchargeai.in/docs)**

## Install

```bash
curl -fsSL https://www.superchargeai.in/install.sh | bash
```

Or install with npm:

```bash
npm i -g @supercharge-cli/supercharge
```

## Get Started

```bash
# Launch the interactive TUI
supercharge

# Run a single task
supercharge -p "Explain this codebase"
```

Configure your model provider's API key before first use (see the [documentation](https://superchargeai.in/docs)):

```bash
export SUPERCHARGE_API_KEY="your-api-key"
```

## Update

```bash
supercharge update
```

Or if installed via npm:

```bash
npm i -g @supercharge-cli/supercharge@latest
```

## Supported Platforms

| Platform | Architecture |
|---|---|
| macOS | Apple Silicon (arm64) |
| Linux | x86_64, arm64 |
| Windows | x86_64 |

## Documentation

For full documentation including configuration, MCP servers, custom models, headless mode, agent mode, and more, visit [superchargeai.in/docs](https://superchargeai.in/docs).

## Feedback

Run `/feedback` inside Supercharge to report issues or send feedback directly.
