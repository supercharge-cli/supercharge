# Daemon and gateway requirements

The gateway target is a long-running process, not merely an HTTP health server. Hermes documents a unified gateway that connects multiple platforms, normalizes inbound events, routes them to sessions, handles slash commands and approvals, persists pairing/session state, delivers scheduled results, and performs background maintenance.

| Requirement | Source-grounded behavior | Rust implementation target |
| --- | --- | --- |
| Daemon lifecycle | One long-running process with clean shutdown, restart supervision, and periodic maintenance | `superagent daemon` mode, systemd/Docker supervision, SIGTERM handling, readiness and liveness |
| Telegram transport | Bot API over HTTPS; long polling and webhook are mutually exclusive | Start with long polling for private always-on hosts; add webhook verification as an optional mode |
| Telegram auth | Bot token plus numeric allowed-user/chat IDs; default deny is safest | Environment/config allowlists, constant-time token handling, no allow-all default |
| Telegram groups | Privacy mode, mentions, replies, topic/thread IDs, and per-chat routing matter | Normalize chat type, chat ID, thread ID, mention/reply state before session dispatch |
| Session routing | Hermes uses `agent:main:{platform}:{chat_type}:{chat_id}` with thread-aware variants | Rust `SessionKey` type rather than hand-built strings |
| Message guard | Queue messages while a session is active; allow control commands such as stop/approve/deny through an inline path | Per-session serial queue plus interrupt/control channel |
| Pairing | Pairing codes authorize new users and persist across restarts | SQLite-backed pairing store with expiry, single-use codes, and audit events |
| Delivery | Replies, scheduled results, explicit target delivery, and cross-platform routing | `GatewayMessage` and `DeliveryTarget` abstractions |
| Media | Telegram supports text, voice, images, files, and generated attachments | First text-only transport; add file/media metadata and safe host-path delivery later |
| Slack/Discord | Socket/WebSocket or platform-specific long-lived connections; each has its own token and event rules | Implement adapters behind the same normalized event/delivery traits after Telegram is stable |
| Background maintenance | Cron tick, session expiry, memory flush, cache refresh | Tokio supervisor task with cancellation and durable job leases |

## Security constraints

Gateway credentials must be supplied through runtime secrets and never stored in Git. Telegram webhook mode must validate the `X-Telegram-Bot-Api-Secret-Token` header. Polling mode must maintain and persist the update offset so restarts do not replay messages. Incoming users and chats are denied unless allowlisted or explicitly paired. Group messages should require an explicit mention or reply by default.

## Sources

The feature baseline was derived from the Hermes Telegram guide, Hermes gateway internals guide, Hermes Slack guide, and the official Telegram Bot API reference. Telegram documents `getUpdates` and webhooks as mutually exclusive delivery mechanisms, and Telegram updates are retained only for a bounded period, so the daemon must persist its own offset and avoid relying on Telegram as a durable queue.

[1]: https://hermes-agent.nousresearch.com/docs/user-guide/messaging/telegram "Hermes Telegram integration"
[2]: https://hermes-agent.nousresearch.com/docs/developer-guide/gateway-internals "Hermes gateway internals"
[3]: https://hermes-agent.nousresearch.com/docs/user-guide/messaging/slack "Hermes Slack integration"
[4]: https://core.telegram.org/bots/api "Telegram Bot API"
