# Supercharge CLI outbound services and credential boundaries

This inventory documents where the CLI may send data and which credential source is allowed. It is intentionally provider-neutral: `x.ai/*` ACP method names may remain for wire compatibility, but they are not proof of an HTTP request to xAI.

| Surface | Destination | Credential source | Implicit fallback |
| --- | --- | --- | --- |
| Main model / `/v1/models` | The selected model's configured `base_url`, `models_base_url`, or explicit provider endpoint | The selected model's `api_key` / configured `env_key`, explicit deployment key, or an explicitly selected session/OIDC provider | No ambient `XAI_API_KEY` or `XAI_CODE_API_KEY`; no automatic endpoint substitution |
| Nous/OIDC login | The explicitly configured identity provider / auth backend | User-selected OAuth/OIDC flow or cached Supercharge auth for that configured scope | No generic fallback from a configured API provider to browser login |
| Interactive ACP login methods (`cached_token`, `supercharge.ai`, `oidc`) | The configured OIDC/OAuth2 issuer or external auth command | Only advertised and accepted when that authority is configured | Provider-only launches advertise none of them and refuse the method ids |
| MCP HTTP/SSE | The MCP server URL in the active MCP configuration | The MCP server's configured headers, bearer env var, or OAuth registration | No model credential reuse |
| MCP gateway connectors | The explicitly configured gateway URL | Gateway-specific configured auth | No model credential reuse |
| Web search, media, voice, compaction, suggestions, and memory side calls | The active provider's configured endpoint when supported | The active provider/model credential or session-local provider route | No implicit xAI side service for custom providers |
| Telemetry / traces / feedback | Configured Supercharge telemetry or explicit OTLP/trace endpoint | Deployment/configured telemetry credentials | No model API key reuse |
| Updates | Supercharge release/update endpoints | No model credential | Not an inference path |

## Secret guarantees

- Ambient first-party key variables are compatibility names only. The runtime helpers intentionally report them as unavailable, so setting `XAI_API_KEY` or `XAI_CODE_API_KEY` cannot select authentication.
- Account session auth is opt-in. `GrokComConfig::session_auth_enabled()` is true only when the deployment configures OIDC, OAuth2, or an external auth command; the provider-first runtime default (`GrokComConfig::provider_only_default()`) configures none of them and uses a non-matching `auth.json` scope.
- While session auth is off, `AuthManager` never resolves the inherited first-party scopes in `auth.json`, `AuthManager::new` does not prune that file, ACP `initialize` does not reload an account token or run a silent refresh, and `authenticate` refuses `cached_token` / `supercharge.ai` / `oidc` with `/setup` guidance. A missing provider credential therefore leads to `/setup`, never to a first-party sign-in.
- An explicit logout still clears those inherited entries (`AuthManager::clear_inherited_scopes`), so saying "log out" is never a no-op while a first-party credential sits on disk.
- Provider secrets configured through `/setup` or a model entry are resolved only for that model and endpoint. Their raw values are not written to model catalogs or cache identity files; cache scopes use a digest.
- Legacy auth data is not copied into Supercharge configuration automatically. Cached session auth is scoped and cannot be sent to a third-party model endpoint by the credential guard.
- Logs and diagnostics must use redacted auth metadata. Tests use fake keys and local mock servers; never use real credentials in repository tests.

When adding a new outbound surface, update this file and add a deterministic test for its credential source, destination, and no-fallback behavior.
