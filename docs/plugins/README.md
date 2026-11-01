# Self-hosted plugins

Supercharge owns plugin execution and credentials on each agent. Desktop and CLI use the same ACP operations. Local and remote agents have separate installations and account stores. No Composio account or Plugin API service is needed.

## Packages and installation

Portable packages contain root `plugin.json`, `mcp.json` and optional `skills/<name>/SKILL.md`. The MCP file accepts `mcpServers` or portable `servers`. Existing `.supercharge-plugin`, `.claude-plugin` and `.mcp.json` layouts remain supported. An explicit manifest MCP path takes precedence; otherwise `mcp.json` precedes `.mcp.json`. Portable tool server identities are namespaced by plugin. Legacy names retain compatibility.

The shared agent defaults to the hosted catalog at `https://www.superchargeai.in/api/plugins/v1`; without a catalog token it lists only public releases alongside bundled packages. Private releases require the owner-provided read token. Configure it using `supercharge plugin catalog configure --token-stdin` (send the token through protected stdin), or through Studio’s Plugins view. `supercharge plugin catalog clear` disables the hosted catalog and removes its saved token; run configure to reconnect. Custom catalogs and environment overrides remain supported and scoped to the selected agent. If the default catalog is unavailable, bundled plugins remain available.

The built-in GitHub and Microsoft Learn packages are embedded at version 1.0.0 in the agent binary. Their package bytes are tied to the binary revision, rather than a floating Git branch. Remote provider server implementations can evolve independently. Skills are original Supercharge workflows; MCP providers are GitHub and Microsoft. Microsoft Learn needs no account. GitHub includes a draft-only Release Brief skill.

In Studio type `/plugins` to review/install packages and enable them separately. `/connections` configures accounts and shows tool health. Paths in Install from source are paths on the selected agent. Git URLs use the existing acquisition and managed-policy gates. Source installs must be reviewed before installation because packages can contain executable hooks/tools.

CLI commands:

```sh
supercharge plugin catalog
supercharge plugin agent plugins/catalog
printf '%s\n' '{"action":{"type":"install","source":"starter:github"}}' | supercharge plugin agent plugins/action --stdin
supercharge plugin agent plugins/list
```

Use the installed plugin `id` from the list with `plugins/action` actions `enable`, `disable`, `update`, or `uninstall` (`plugin_id`). `reload` applies registry changes; the app does this when required. Management installs are disabled by default. Existing `plugin install --trust` retains its explicit activation behavior.

## Account connections

In Studio Connections, enter the plugin name and the server key in its package (`github` / `supercharge-github` for GitHub), an account name, and either an access token or your OAuth app client ID, optional client secret and scopes. Use named accounts such as work and personal. Select an account explicitly. Accounts and OAuth connection identities are bound to plugin, server, complete resource URL and account name.

For CLI account setup, pass a JSON object through stdin to `plugin agent connections/configure`:

- `plugin`, `server`, `account`
- Token authentication: `secretKind: "bearer"`, `secret`
- OAuth: `secretKind: "oauth"`, `clientId`, optional `scopes`
- Confidential OAuth: `secretKind: "client_secret"`, `clientId`, `secret`, optional `scopes`

Do not put real tokens in shell history. Supply JSON from a protected file descriptor or secret manager. Never pass secrets as command-line arguments. An API key is supported when its MCP endpoint accepts bearer authentication; arbitrary provider-specific API-key headers are not generated automatically.

`connections/list` returns redacted account metadata, including storage backend and selection. `connections/select` takes `id`; `connections/disconnect` takes `id`. Disconnect clears locally held credentials and reloads connections. Provider-side revocation is reported as unsupported; remove the grant in the provider account settings as well.

`mcp/list` returns target tool server names and health. Use its runtime server name with `connections/connect`, `connections/status`, and `connections/cancel`. Configure/select the account before enabling an authenticated plugin to avoid an initial unauthenticated probe. OAuth discovery and refresh use the existing Rust MCP implementation. Providers without dynamic client registration need your app registration; installing a package does not create it.

Account token/application secrets use the OS keychain where available; the fallback is an atomic owner-only `plugin-connections.json` under `SUPERCHARGE_HOME`. The fallback is not encrypted. Existing OAuth tokens remain in the runtime's owner-only `mcp_credentials.json`, protected by its cross-process locking. Neither file belongs in Git. Account settings never go into Studio localStorage or plugin manifests.

## Remote agent and OAuth callbacks

Remote targets are personal agents, authenticated with the existing owner key. Run the agent listener on loopback behind an HTTPS/WebSocket reverse proxy. Raw non-loopback WebSocket connections are refused. Do not expose the plain agent listener publicly or trust forwarded headers to select an OAuth callback origin.

Configure on the agent host:

```sh
export SUPERCHARGE_MCP_OAUTH_CALLBACK_URL=https://agent.example.com/callback
export SUPERCHARGE_MCP_OAUTH_CALLBACK_PORT=2420
supercharge agent serve --bind 127.0.0.1:2419 --secret '<owner-key>'
```

Replace the example hostname and key. Configure your OAuth provider's allowed redirect URI to the exact HTTPS URL. Register a separate localhost redirect/app where the provider requires it for local desktop usage. The callback listener is temporary and binds only 127.0.0.1:2420 during consent. A fixed callback port permits one browser consent flow at a time; other attempts receive a bind error and can retry.

Example Caddy configuration (requires a domain pointing at this host):

```caddy
agent.example.com {
    @callback path /callback
    handle @callback {
        reverse_proxy 127.0.0.1:2420
    }
    handle /ws {
        reverse_proxy 127.0.0.1:2419
    }
    handle {
        respond "Not found" 404
    }
}
```

Keep callback query strings out of proxy access logs. The callback must remain reachable through provider redirects; its one-use state and PKCE protect the exchange. Consent expires after ten minutes. The app provides a sign-in link and cancellation; credentials are exchanged/stored on the target.

CLI remote operations require `--agent wss://agent.example.com/ws --cwd /path/on/agent` and `SUPERCHARGE_AGENT_KEY` in the environment. Credentials in URL user-info/query strings are refused. The client never falls back to a local target. Browser-based Studio uses the configured agent connection and displays its redacted target address.

## Verification

Automated checks cover package precedence/containment, account identity, concurrent atomic storage and redaction, callback URL validation, frontend transport/error boundaries and desktop lifecycle against an ACP fixture. Live provider verification is separate: it requires the configured provider app/account, TLS domain and remote host. A successful mocked sign-in is not evidence of a successful live OAuth exchange.

References: https://developers.openai.com/plugins/build/plugins, https://github.com/MicrosoftDocs/mcp, https://github.com/github/github-mcp-server/blob/main/docs/host-integration.md, https://docs.composio.dev/docs/how-composio-works.
