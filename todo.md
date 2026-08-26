# E2B Browser-Control Follow-up

- [x] Inspect the E2B-only agent system prompt and native browser-tool guidance for explicit browser-control behavior.
- [x] Add clear instructions for the agent to request browser-tab handoff only for login, MFA, CAPTCHA, OAuth, or other sensitive user input.
- [x] Verify noVNC remains ticket-gated, browser-tab-only, and unavailable to the agent or public clients.
- [x] Run focused regression checks, then commit and push the codebase-only refinements.
- [x] Reproduce and diagnose the app-driven terminal command flow being reported as a user cancellation: daemon app turns omitted the headless auto-approval flag, leaving tool permission prompts unanswerable.
- [x] Ensure an app turn runs with headless tool approval so terminal actions execute instead of becoming unanswerable cancellation prompts; update daemon E2B rules to the native browser and secure handoff model.
- [ ] Verify the command-recovery fix through focused daemon and app-flow regressions, then commit and push it.
