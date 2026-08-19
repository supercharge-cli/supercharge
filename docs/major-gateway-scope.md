# Major daemon and gateway scope

The daemon will use a normalized `GatewayMessage` and `DeliveryTarget` contract so platform-specific transport behavior stays separate from the Rust agent/session engine.

| Platform | Inbound transport | Outbound transport | Important daemon requirements |
| --- | --- | --- | --- |
| Telegram | Bot API long polling or webhook | Bot API HTTPS methods | Persist update offset; allowlists; optional mention/topic handling; webhook secret validation |
| Discord | Persistent Gateway WebSocket | Discord HTTP API and Gateway | Identify with intents; heartbeat/ACK; sequence tracking; resume/reconnect; message-content intent; rate limits; guild/channel allowlists |
| Slack | Socket Mode WebSocket | Slack Web API and Socket Mode responses | Refresh WebSocket URLs; acknowledge envelopes; bot/app tokens; workspace/channel allowlists; reconnects |
| WhatsApp Business | HTTPS webhooks | Graph API HTTPS calls | Verify webhook challenge; validate signatures; deduplicate retries; phone-number and business-account configuration |
| Signal | Adapter-specific daemon/API | Adapter-specific HTTP/IPC | External signal-cli or compatible service; durable delivery and reconnect policy |
| Matrix | Client-server sync or application service | Client-server HTTP | Persist sync token; room/user allowlists; E2EE is a separate scope |
| Email | IMAP IDLE/polling | SMTP | Credential storage; mailbox cursor; threading and attachment limits |
| Generic webhook | HTTPS inbound | Configurable HTTP outbound | Request authentication/signatures; replay protection; idempotency keys |

## Shared daemon requirements

The daemon must run a supervisor around every adapter with cancellation, bounded reconnect backoff, structured health status, and per-adapter last-success timestamps. Inbound events must be normalized before authorization and session routing. Each session needs a durable key containing platform, chat, and thread context. Active sessions require serialized processing and a control path for stop, approve, deny, and status operations.

Discord’s official Gateway documentation requires a persistent WebSocket lifecycle with Hello, heartbeats, ACK handling, Identify, sequence numbers, and resumable reconnects. Gateway intents determine which event families are delivered, and message content requires the appropriate intent and application configuration [1] [2]. Slack Socket Mode creates a runtime WebSocket URL through `apps.connections.open`, requires envelope acknowledgements, and expects connection refresh/reconnect handling [3]. WhatsApp Business delivers webhook retries for up to seven days when the endpoint does not return a successful response, so webhook ingestion must be idempotent and deduplicate event IDs [4].

## Implementation order

The recommended order is: first finish the durable session/job core; then complete Telegram; then implement Discord because it is a persistent gateway with heartbeat/resume complexity; then Slack Socket Mode; then webhook-based WhatsApp and generic webhook support; then Matrix and email; finally adapters requiring external daemons or vendor-specific infrastructure.

## References

[1]: https://docs.discord.com/developers/events/gateway "Discord Gateway documentation"
[2]: https://docs.discord.com/developers/events/gateway-events "Discord Gateway events"
[3]: https://docs.slack.dev/apis/events-api/using-socket-mode "Slack Socket Mode"
[4]: https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/overview "WhatsApp Business webhooks"
