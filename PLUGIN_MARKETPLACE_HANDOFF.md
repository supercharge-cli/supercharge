# Plugin Marketplace — Handoff & Context (2026-10-03)

**Purpose:** Complete context for continuing this work in a new chat or with a different
coding tool. Read this before touching any file.

---

## 1. THE PRODUCT VISION (user's clarified requirement — build this first)

The user does **not** want the current "Managed MCP" settings section as the primary
surface. That was an implementation detail. The product is a **Plugin Marketplace**,
like the Play Store:

- A first-class **Marketplace tab/screen** in Vectra mobile, Vectra web, and Supercharge
  desktop — not buried in workspace settings.
- **Store-style cards**: plugin **logo/icon**, name, publisher, short description,
  version, category, permissions summary, install button. A **detail page** per plugin
  with full description, permissions breakdown, and target (VM/bot) selection.
- **Install flow** (per plugin): pick owned VM/bot → pinned release is downloaded and
  installed into that VM's isolated Plugin API store (disabled, untrusted) → if the
  plugin declares auth/secrets, the user completes **OAuth or write-only secret entry** →
  explicit **permission approval** → enable → **attach to a conversation** (MCP bridge)
  → verify with a safe tool call.
- The install **gets saved** (per user/instance) and **ships to the VM** — the VM runs
  the plugin; the app is only a controller.
- The user's words: "plugin marketplace with their logo and all info like play store
  type, they install, do auth if needed, and then it gets saved that gets shipped to
  the VM".

**What already exists that this builds on** (do not delete): the curated catalog, the
pinned-archive install path into the VM, and the review/attach flows below. The
marketplace UI is a new layer over them; the existing Managed MCP settings screens
remain as the "manage installed plugins" surface.

---

## 2. REPOSITORIES (all under `/mnt/main-ssd/home-data/codes/supercharge/`)

| Repo | Remote | Role |
|---|---|---|
| `.` (root) | `supercharge-cli/supercharge` | Sandbox template, Rust daemon/CLI, `plugin-api/` service source |
| `supercharge-app/` | `supercharge-cli/supercharge-app` (PUBLIC) | Desktop app (Tauri + React); also hosts the **public catalog assets** |
| `cloud-infra/` | legacy remote unavailable | Vectra web (Next.js, Vercel) + mobile (Flutter) + gateway |

All three were pushed to `main` this session.

---

## 3. WHAT WAS BUILT THIS SESSION (files + revisions)

### 3.1 Shared curated catalog (the "store inventory")
- `cloud-infra/apps/web/public/managed-mcp/catalog.json` — catalog listing. One entry:
  `vectra-echo` v1.0.0, pinned archive `https://raw.githubusercontent.com/supercharge-cli/supercharge-app/379e0d2789ac918e327e5ef1fef5cc2991a431a7/public/managed-mcp/vectra-echo-1.0.0.tar.gz`, SHA-256 `a0c6be6c2f292c1a1ecdcc55056147d1dd232b58c76522e77ad44016ee7c0072`.
- `cloud-infra/apps/web/public/managed-mcp/echo-1.0.0/{plugin.json,mcp.mjs}` — reviewed
  release source (echo-only stdio MCP, `node ${PLUGIN_ROOT}/mcp.mjs`, permission
  `mcp:stdio`, no hooks/components/secrets).
- `cloud-infra/apps/web/scripts/validate-managed-mcp-catalog.mjs` — build gate
  (`npm run catalog:check` in apps/web): verifies IDs, pinned first-party URL format
  (`raw.githubusercontent.com/supercharge-cli/supercharge-app/<40-hex-sha>/public/managed-mcp/…`),
  archive checksum, exact reviewed file set, manifest↔catalog metadata match.
- Published **public immutable copies** (mobile/desktop read these directly):
  - catalog: `https://raw.githubusercontent.com/supercharge-cli/supercharge-app/de50dfa59e9b7b7d2ee77b3e1f732242ac84e012/public/managed-mcp/catalog.json`
  - reviewed manifest (desktop hydrates it): `https://raw.githubusercontent.com/supercharge-cli/supercharge-app/6204686983497b32b798376b6ea6ed5e105be182/public/managed-mcp/echo-1.0.0/plugin.json`
- NOTE: the tar was repacked **without directory entries** (`tar --format=ustar -czf … mcp.mjs plugin.json`)
  because the Plugin API extractor rejects archives containing `./` entries
  (`UNSAFE_ARCHIVE`). Keep new packages built the same way.

### 3.2 Vectra mobile (Flutter) — `cloud-infra/apps/mobile/`
- `lib/api/client.dart` — `managedMcpCatalog()` now fetches the **public immutable
  catalog.json directly** (no auth header; read-only). Deliberate: the live Vectra web
  is an old build and does not have `/api/managed-mcp/catalog` yet.
- `lib/screens/managed_mcp_section.dart` — in Workspace settings → "Managed MCP":
  bot selector, conversation state, **Shared reviewed catalog** dropdown (Echo MCP
  visible on the phone now), "Connect: install in this VM" button, existing-conversation
  selector, Apply/Retry/Remove, plus configure (write-only JSON), permission-approval
  checkbox, Trust, Enable. Catalog load failures show a distinct red `catalogError`
  line (no longer silently cleared by status refresh).
- Test updated: `test/browser_api_test.dart` ("Managed MCP catalog is read-only from
  the public source…"). All Flutter tests pass; `flutter analyze` clean.

### 3.3 Vectra web (Next.js) — `cloud-infra/apps/web/`
- `app/api/instances/[id]/bots/[botId]/plugin-api/route.ts` — owner-checked proxy now
  accepts `installCatalog` and `pluginAction: update` **only** when
  `resolveCatalogEntry(catalogId, catalogSha256)` matches the bundled catalog; 512 KiB
  body bound. **Not deployed yet** (see blockers).
- `app/api/managed-mcp/catalog/route.ts` — GET the bundled catalog (also not deployed).
- `lib/managed-mcp/catalog.ts` + `lib/managed-mcp/catalog.test.ts` — `resolveCatalogEntry`,
  refuses unknown/manifest-mismatched entries.
- `components/managed-mcp.tsx` — web card gained the catalog section, conversation
  dropdown (real conversations instead of pasted IDs), catalog update button.

### 3.4 Supercharge desktop — `supercharge-app/`
- `src-tauri/src/commands/plugin_api.rs` — new Tauri command `plugin_api_catalog`
  (registered in `src-tauri/src/command_registry.rs`): fetches the pinned public
  catalog.json, then hydrates each entry with the reviewed `plugin.json` manifest from
  the pinned manifest URL; rejects unknown entries.
- `src/lib/api/pluginApi.ts` — `pluginApiCatalog()` + `PluginApiCatalogEntry`.
- `src/components/PluginApiCatalogSection.tsx` (new) + `src/components/PluginApiPanel.tsx`
  — catalog section above the installed inventory; installs the pinned archive through
  the existing desktop Plugin API adapter. Extracted to a separate file because desktop
  CI enforces a file-length budget (see §6).
- `src/i18n/pluginApi.ts` — `pluginApi.catalog.*` keys (en + fallbacks).

### 3.5 Sandbox / VM (root repo)
- `sandbox/e2b-agent-template/plugin-api-control.mjs` — root-only controller gained
  `installCatalog` action and `pluginAction: update` via `catalogSource()`: reads
  `/opt/vectra/plugin-api/catalog.json` baked into the image, matches id+sha256,
  allows only `raw.githubusercontent.com/supercharge-cli/supercharge-app/<sha>/public/managed-mcp/*.tar.gz`,
  installs through Plugin API with `source.type=archive` (digest verified again at
  staging). Keeps the UID-1000/1001 fix from earlier this week.
- `sandbox/prepare-e2b-agent-assets.sh` — copies `catalog.json` into the template's
  `/opt/vectra/plugin-api/`.
- `crates/codegen/supercharge-superagent/src/bin/server/plugin_api.rs` — typed daemon
  action `InstallCatalog { catalogId, catalogSha256, requestId }` forwarded to the
  controller. `cargo test` (29 tests) passes.
- `sandbox/tests/plugin_control_uid_smoke.sh` — extended: proves `installCatalog`
  installs `vectra-echo` (disabled+untrusted) into the isolated per-bot Plugin API
  service inside Docker fixtures for **both UID 1000 and UID 1001**, and rejects a
  wrong digest. **PASS.**

### 3.6 Commit map
- Root repo: `b3948fbc8` (UID-aware controller), `561b5ddc5` (catalog controller+daemon).
- cloud-infra: `59ee295` (catalog feature web+mobile), `5b83a09` (catalog error surfaced),
  `2b31949` (mobile reads public source).
- supercharge-app: `f10a3af8`, `28a36559`, `5028c1dd`, `e503b1f7`, `de50dfa5`,
  `379e0d27`, `62046869`, `c256f39d`, `c9e880eb` (catalog + asset publishing + budget fix).

---

## 4. TEST STATUS (what is verified vs not)

**Verified:**
- Catalog validation gate, web typecheck/lint, `npm run build` (full Next production
  build) all pass.
- Vectra mobile CI (GitHub Actions) passed all platform jobs for `59ee295`.
- Desktop CI passed all jobs (frontend, Rust Linux/Windows/macOS) for `c9e880eb`.
- Root repo `cargo test` daemon suite passes with the new typed action.
- Docker fixtures: controller installs the pinned echo archive into the isolated
  per-bot Plugin API service on UID 1000 and 1001; digest mismatch rejected.
- **On the connected phone** (Realme RMX5398, serial `3H166R0040B00000`, package
  `cloud.vectra.vectra_cloud`): latest debug APK installed **over** the existing app
  with the matching certificate (`606565a76615ea72a7546292259f7735d2c27e4abf4f72042e38b1e7e68489b3`),
  no uninstall, firstInstallTime preserved. Signed in, workspace "running", custom AI
  provider (gpt-6-sol · responses · https://gpt.n92dev.us.kg) configured, chat replies
  worked earlier on sandbox `iirumdifrmp1dl03bk1se`. **The Shared reviewed catalog now
  renders in the app: "Catalog plugin · Echo MCP · 1.0.0" with pinned SHA-256 and an
  enabled "Connect: install in this VM" button.**

**NOT verified / blocked:**
- **Connect (install) on the phone was tapped but the result was never observed**
  (session paused right after the tap). Expected outcome given blockers below: an
  honest error from the stale web route ("Unsupported managed MCP action"). Re-test
  after deployment.
- **Vercel production is stale AND the team is blocked**: `vercel --prod` fails with
  "Your Team exceeded our fair use limits and has been blocked"; GitHub deployment
  status also shows "Account is blocked". The live site `https://infra.xibeai.in`
  serves an **old build**: `/api/managed-mcp/catalog` → 404, and the bot proxy route
  rejects `installCatalog`. The previous Vercel account is unavailable, so deployment
  requires a new authenticated account and project target.
- **E2B template not republished** with the catalog controller (`561b5ddc5`). Published
  alias `supercharge-full-sandbox` (id `i555ld3tcet21gyd3mry`) still has the
  UID-fix controller but **no** `catalog.json`/`installCatalog`, so even a fresh
  sandbox would reject the archive install. Existing sandbox VMs are never migrated by
  a template publish.
- `gpt-6-luna` is rejected upstream ("no upstream owns model GPT-6-luna", and a direct
  `/responses` probe returned HTTP 403 code 1010). The user switched to `gpt-6-sol`,
  which works. Do not claim luna support.
- Desktop **release** with the catalog feature has not been tagged/built.

**To finish the current design end-to-end (in order):**
1. Unblock/fix the Vercel account, then deploy cloud-infra `apps/web` (CLI `vercel --prod`
   or push-triggered). Verify `https://infra.xibeai.in/api/managed-mcp/catalog` → 200 JSON.
2. Rebuild + publish the E2B template (`sandbox/publish-e2b-template.sh`) so new
   sandboxes contain `catalog.json` + the `installCatalog` controller; the user creates
   a **new** sandbox (or we implement an in-sandbox controller update path).
3. On the phone: refresh status → select Echo MCP → "Connect: install in this VM" →
   configure (none needed for echo) → approve permissions → enable → pick an existing
   conversation → Apply → then run the web dashboard's consented echo verification.
4. Cut desktop + mobile releases if the flow passes on the phone.

---

## 5. THE NEW MARKETPLACE WORK — design sketch to implement next

Catalog format (extend, don't break): add per-entry `icon` (URL), `publisher`,
`category`, `screenshots` (optional), `auth: "none" | "oauth" | "secrets"` +
`userConfig` summary, `targets` (supercharge-e2b). Keep `source.type=archive` +
`sha256` pinning and the validation gate (extend it to check icon URL format and auth
enum). Update `catalog:check`, web `catalog.ts`, desktop hydration, and the mobile
parser together.

UI:
- **Vectra mobile**: new Marketplace screen reachable from the drawer (first-class item
  next to Chats/Files), grid of cards (icon, name, publisher, one-liner, Install
  button), detail page with full info + "Install to <VM/bot>" picker, then a stepper:
  install → auth/secrets (if `auth != none`) → review permissions → enable → attach to
  conversation. Reuse `botManagedMcp` actions; add no new credentials to the phone.
- **Vectra web**: marketplace page under the dashboard for Supercharge instances,
  same stepper; it owns the review/verify surfaces.
- **Supercharge desktop**: Marketplace section in the Managed MCP panel or a new tab;
  install targets the connected local Plugin API service (existing adapter) and can
  deep-link Vectra for VM installs later.
- **Saved state**: per-instance installed inventory already comes from the VM
  (`status` → `inventory.plugins`); "My apps" = that inventory joined with catalog
  metadata by id. No server-side store needed initially.
- **Auth**: for `oauth`, add a per-plugin OAuth callback/config step in the web
  dashboard writing the resulting secret via write-only `configure`; the phone shows
  status only. For `secrets`, use the existing write-only config fields.

Security invariants to keep (do not relax):
- No Plugin API management tokens or sandbox control secrets in the mobile app, browser
  bundle, or agent.
- Installs only from the pinned, checksum-verified first-party archive; the controller
  allowlist (raw.githubusercontent.com/supercharge-cli/supercharge-app/<sha>/…) stays.
- Install starts disabled + untrusted; explicit permission approval before enable;
  separate consented verification before "ready".
- Per-bot isolated Plugin API store/service UID; per-conversation bridge credentials.

---

## 6. ENVIRONMENT / COMMANDS CHEAT SHEET

- Memory limit: keep Gradle/Java bounded — `JAVA_TOOL_OPTIONS=-Xmx3g flutter build apk --debug`
  in `cloud-infra/apps/mobile` (host has had RAM pressure).
- Phone: serial `3H166R0040B00000`; verify cert with
  `~/.android/debug.keystore` (SHA-256 `6065…89b3`) via
  `Android/Sdk/build-tools/35.0.0/apksigner verify --print-certs`.
- E2B CLI: source `E2B_API_KEY` from `/mnt/main-ssd/home-data/codes/supercharge/.env.runtime`
  (don't print it). Template alias `supercharge-full-sandbox` (id `i555ld3tcet21gyd3mry`),
  16 CPU / 16384 MB.
- Catalog gate: `npm run catalog:check` in `cloud-infra/apps/web`.
- Web tests: `esbuild <test>.ts --bundle --platform=node --format=esm --packages=external`
  then `node --test` (no tsx in apps/web); or `node --test --experimental-strip-types`
  for simple ones.
- Desktop quality gate: `python3 scripts/check-code-quality-gates.py --mode final` in
  `supercharge-app/` — **FILES_OVER_1K_BUDGET caps files ≥1000 lines at 80**; that is
  why `PluginApiCatalogSection.tsx` was extracted. Run it locally before pushing.
- Vercel CLI installed (`~/.bun/bin/vercel`), but the previous account is unavailable;
  authenticate a new account and project target before deploying.
- Flutter at `/home/r3ap3reditz/flutter/bin/flutter`; adb at `/usr/sbin/adb`.
- Known bad: `gpt-6-luna` upstream; direct curl to the user's proxy from the sandbox
  gets Cloudflare 403/1010 — test models through the app/daemon, not raw curl.

---

## 7. CURRENT PHONE STATE (leave as-is unless asked)

- App updated in place (v1.0.27 / versionCode 28, lastUpdate today). Signed in as the
  user's account. Workspace "running", 1 agent (Main), custom provider saved.
- Managed MCP section shows the loaded catalog. "Connect" was tapped once, outcome
  unobserved; no plugin state is known to have changed in the VM (old controller would
  reject it; the web would reject it first).
- Nothing destructive was done; no other users' sandboxes touched.
