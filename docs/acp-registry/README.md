# ACP Registry submission

Copy `supercharge/` into a fork of https://github.com/agentclientprotocol/registry and open a PR.

**Do not submit until a public release advertises ACP `authMethods`** (agent OAuth `supercharge.ai` and/or `terminal-setup`). Registry CI runs `initialize` on the downloaded binary and rejects an empty list. Current GitHub release `v1.2.5` is the binary layout and checksums; bump `version` and asset URLs after you ship the auth handshake.

Until the PR is merged, users get Supercharge in Zed with:

```bash
supercharge acp setup --editor zed
```
