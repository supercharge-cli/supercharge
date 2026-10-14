# Supercharge × SkillMD Plugin Marketplace Integration Spec

## Goal

Integrate SkillMD's public Agent Skill / Plugin registry into **Supercharge** as a marketplace source.

Supercharge already has a real plugin architecture, Plugin API installation flow, skills discovery, MCP integration, plugin registry, and live plugin reload support. This work should add SkillMD as an external marketplace/catalog source without bypassing Supercharge's existing security and installation pipeline.

---

# 1. Existing Supercharge architecture

Repository:

`supercharge-cli/supercharge`

Important existing configuration:

```env
SUPERAGENT_SKILLS_ROOT=/var/lib/superagent/skills
SUPERAGENT_SKILL_REQUIRE_TRUST=false
SUPERAGENT_MCP_SERVERS=
SUPERAGENT_MCP_STDIO_ALLOWLIST=
```

The skills root is configurable through `SUPERAGENT_SKILLS_ROOT`; do NOT hard-code `/var/lib/superagent/skills` in Rust when the environment/config already provides the root.

The existing codebase contains:

- PluginRegistry
- plugin discovery
- plugin-contributed skills
- plugin-contributed MCP servers
- plugin hooks
- plugin-aware agent lookup
- Plugin API service/controller
- managed MCP state
- MCP bridge
- plugin reload / live session refresh
- plugin installation into isolated VM/bot Plugin API stores
- disabled/untrusted installation state
- permission approval
- per-conversation MCP attachment

The existing session code also supports refreshing the on-disk plugin registry so newly installed plugins can become visible to live sessions.

There is already a plugin marketplace handoff/design in:

`PLUGIN_MARKETPLACE_HANDOFF.md`

Do not replace the existing Plugin API architecture. Extend it.

---

# 2. What SkillMD provides

SkillMD is an Agent Skill registry.

A normal Agent Skill is based around:

```text
<skill-directory>/
└── SKILL.md
```

SkillMD's public API is available at:

```text
https://api.skillmd.com
```

The public registry API documented by SkillMD includes:

```http
GET /v1/search?q=<query>
GET /v1/skills/<owner>/<name>
```

SkillMD's own open-source repository states that its CLI is a thin client over this public API.

Useful public documentation:

- https://skillmd.com/docs
- https://skillmd.com/plugins
- https://github.com/skillmds/skillmd

---

# 3. Skill search API

Example:

```bash
curl -s 'https://api.skillmd.com/v1/search?q=pdf'
```

The search response contains skill metadata and a raw URL for the installable `SKILL.md`.

Supercharge should NOT require the SkillMD CLI just to search.

Preferred implementation:

```text
Supercharge Rust
      |
      | HTTPS GET
      v
https://api.skillmd.com/v1/search?q=pdf
```

Parse the JSON into an internal type such as:

```rust
struct SkillMdSearchResponse {
    // Match the actual API response fields.
    // Keep unknown fields ignored for forward compatibility.
}

struct SkillMdSkill {
    owner: String,
    name: String,
    // metadata fields returned by SkillMD
    raw_url: Option<String>,
}
```

Do not assume every field is mandatory. Treat optional metadata defensively.

---

# 4. Individual skill lookup

For a known skill:

```http
GET https://api.skillmd.com/v1/skills/<owner>/<name>
```

Example pattern:

```bash
curl -s \
  'https://api.skillmd.com/v1/skills/anthropic/pdf'
```

This should be used for the marketplace detail page when the user selects a skill.

The UI should be able to show:

- name
- owner/publisher
- description
- version/update information when provided
- safety information when provided
- repository information when provided
- install information
- supported agents/capabilities when provided

Do not invent metadata that is not returned by the API.

---

# 5. Raw SKILL.md

For a normal skill, SkillMD exposes a raw installable `SKILL.md` URL through registry/search metadata.

The installer should:

1. Fetch metadata.
2. Obtain the raw skill URL.
3. Download the content.
4. Validate it.
5. Place it into a temporary/quarantine directory.
6. Run Supercharge's trust/security checks.
7. Move it into the configured skills/plugin installation location.
8. Trigger the existing plugin/skill registry refresh.

Do not write untrusted downloaded content directly into the active runtime directory.

Target structure:

```text
<skills-root>/
└── <skill-name>/
    └── SKILL.md
```

where `<skills-root>` comes from:

```env
SUPERAGENT_SKILLS_ROOT
```

---

# 6. IMPORTANT: Plugin vs Skill

SkillMD distinguishes:

### Skill

A single reusable Agent Skill.

Typical structure:

```text
my-skill/
└── SKILL.md
```

### Plugin

A bundle of related skills installed together.

SkillMD's plugin marketplace describes plugins as bundles of Agent Skills.

Example:

```text
plugin
├── skills/
│   ├── skill-a/
│   │   └── SKILL.md
│   ├── skill-b/
│   │   └── SKILL.md
│   └── skill-c/
│       └── SKILL.md
└── additional plugin metadata/files
```

The exact contents can vary by plugin.

Therefore:

**Do NOT reduce a SkillMD plugin to one SKILL.md.**

If the user installs a plugin, preserve the complete bundle/files needed by that plugin.

---

# 7. SkillMD plugin marketplace

SkillMD plugin pages are available under:

```text
https://skillmd.com/plugins
```

A plugin has an owner/name identifier.

Example:

```text
plugin:skillmd/docs-writing
```

The SkillMD CLI installs a plugin with:

```bash
npx skillmds@latest add plugin:skillmd/docs-writing
```

The SkillMD GitHub project documents the plugin identifier format:

```text
plugin:<owner>/<name>
```

Supercharge should use the identifier as a logical marketplace ID.

Example internal ID:

```text
skillmd:plugin:skillmd/docs-writing
```

or:

```text
skillmd/plugin/skillmd/docs-writing
```

Pick one stable canonical representation.

---

# 8. DO NOT hard-code an undocumented ZIP endpoint

SkillMD's website exposes a "Download" action for plugins and documents manual installation by downloading the plugin and unzipping its `skills/` folder.

However, the exact current public HTTP endpoint used by the website for the plugin ZIP should be discovered from SkillMD's current OpenAPI/API documentation or the site's download link before implementing a hard-coded ZIP URL.

Do NOT assume that:

```text
/api/skills/<owner>/<name>/bundle
```

or any other bundle URL is permanently supported unless it is confirmed against SkillMD's current API/OpenAPI documentation.

The implementation should therefore have a provider abstraction:

```rust
trait MarketplaceProvider {
    async fn search(&self, query: &str) -> Result<Vec<MarketplaceItem>>;
    async fn get_item(&self, id: &str) -> Result<MarketplaceItem>;
    async fn download(&self, id: &str) -> Result<DownloadedArtifact>;
}
```

Then SkillMD can implement it.

This prevents the rest of Supercharge from depending on undocumented SkillMD internals.

---

# 9. Recommended SkillMD provider API

Create something conceptually like:

```text
crates/.../marketplace/
├── mod.rs
├── provider.rs
└── skillmd.rs
```

Possible model:

```rust
pub enum MarketplaceItemType {
    Skill,
    Plugin,
}

pub struct MarketplaceItem {
    pub id: String,
    pub item_type: MarketplaceItemType,
    pub name: String,
    pub owner: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub icon_url: Option<String>,
    pub source_url: Option<String>,
    pub safety: Option<MarketplaceSafety>,
}

pub struct MarketplaceSafety {
    pub verdict: Option<String>,
    pub capabilities: Vec<String>,
}

pub enum DownloadedArtifact {
    SkillMarkdown {
        content: Vec<u8>,
    },
    PluginArchive {
        bytes: Vec<u8>,
    },
}
```

Use the actual project conventions/types instead of blindly creating these exact structs.

---

# 10. Marketplace UI

Supercharge/Vectra should have:

```text
Marketplace
│
├── Search
│
├── Categories
│
├── Skill cards
│   ├── icon
│   ├── name
│   ├── publisher
│   ├── description
│   ├── safety
│   └── Install
│
└── Plugin cards
    ├── icon
    ├── name
    ├── publisher
    ├── number of skills
    ├── description
    ├── safety
    └── Install
```

Detail page:

```text
Plugin
├── Logo
├── Name
├── Publisher
├── Description
├── Skills included
├── Safety information
├── Permissions/capabilities
├── Source/repository
└── Install
```

For a VM install:

```text
Select VM/Bot
      ↓
Download
      ↓
Verify
      ↓
Install disabled + untrusted
      ↓
Authentication/configuration
      ↓
Permission approval
      ↓
Enable
      ↓
Attach to conversation
      ↓
MCP bridge / plugin tools
      ↓
Safe verification
```

This matches the existing Supercharge marketplace design.

---

# 11. Security requirements

DO NOT bypass the existing Supercharge security model.

Downloaded SkillMD content is external/untrusted.

Installation must follow:

```text
SkillMD
  ↓
download
  ↓
temporary quarantine
  ↓
archive/path validation
  ↓
security/trust checks
  ↓
optional checksum/signature validation
  ↓
install disabled
  ↓
mark untrusted
  ↓
explicit user approval
  ↓
enable
```

Never:

- execute downloaded scripts during marketplace browsing
- automatically enable a downloaded plugin
- expose Plugin API management tokens to the frontend
- expose E2B sandbox control secrets to the frontend
- give a newly downloaded plugin unrestricted credentials
- trust an archive only because SkillMD returned it
- extract archives without path traversal protection

---

# 12. Existing Supercharge pinned catalog

Supercharge currently has a curated/pinned Plugin API catalog.

Its existing architecture verifies:

```text
catalog ID
+
catalog SHA-256
+
allowed first-party archive URL
+
archive digest
```

The existing sandbox controller installs the archive into the isolated Plugin API service and initially keeps the plugin:

```text
disabled
untrusted
```

Do not remove this mechanism.

For SkillMD, the safest initial integration is:

```text
SkillMD marketplace
       ↓
Supercharge marketplace adapter
       ↓
download
       ↓
quarantine
       ↓
Supercharge validation
       ↓
convert/adapt into the existing Plugin API installation path
```

If a SkillMD artifact cannot safely enter the existing Plugin API archive path, stop and return an actionable error rather than weakening the validation.

---

# 13. VM installation

For VM/bot installs, the app/browser should remain a controller.

Preferred architecture:

```text
Mobile/Web/Desktop
       |
       | install request
       v
Supercharge gateway/daemon
       |
       v
VM / E2B controller
       |
       v
Plugin API service
```

Do NOT put Plugin API management credentials in:

- mobile
- browser bundle
- model context
- normal frontend state

The existing Supercharge marketplace handoff explicitly requires this.

---

# 14. Authentication

A marketplace item may require external authentication.

Support the existing Supercharge concepts:

```text
auth = none
auth = oauth
auth = secrets
```

Flow:

### none

```text
Install → permission approval → enable
```

### secrets

```text
Install
→ user enters secret
→ write-only configuration
→ permission approval
→ enable
```

### OAuth

```text
Install
→ OAuth flow in trusted web/dashboard
→ callback
→ write resulting secret/configuration
→ permission approval
→ enable
```

Never return stored secrets back to the frontend after configuration.

---

# 15. Live activation

Supercharge already has plugin/skill/MCP refresh mechanisms.

Do not restart the entire VM merely to load a marketplace installation if the existing registry refresh mechanism can handle it.

After installation:

```text
install
  ↓
registry refresh
  ↓
skills discovered
  ↓
plugin MCP servers merged
  ↓
live session refresh
  ↓
new tools/skills available
```

The existing code contains logic for refreshing plugin-contributed:

- hooks
- MCP
- skills
- slash commands

Use that existing path.

If a specific plugin requires process startup, let the existing plugin/MCP lifecycle start it safely.

---

# 16. MCP integration

A Skill is instructions.

An MCP server is an executable/live tool provider.

A plugin can combine them.

Conceptually:

```text
SkillMD Plugin
├── SKILL.md
│   └── tells agent how/when to use tools
│
└── MCP integration
    └── provides actual live tools
```

Supercharge already supports both.

After installation:

```text
PluginRegistry
   ├── skills
   ├── MCP configs
   ├── hooks
   └── commands
```

The plugin's MCP configuration must still pass Supercharge's MCP allowlisting/security rules.

Do NOT automatically permit arbitrary stdio executables.

Existing configuration:

```env
SUPERAGENT_MCP_STDIO_ALLOWLIST=
```

must continue to be respected.

---

# 17. Error handling

Marketplace errors should be explicit.

Examples:

```text
SKILLMD_SEARCH_FAILED
SKILLMD_ITEM_NOT_FOUND
SKILLMD_DOWNLOAD_FAILED
SKILLMD_INVALID_RESPONSE
SKILLMD_UNSUPPORTED_ARTIFACT
SKILLMD_ARCHIVE_INVALID
SKILLMD_ARCHIVE_PATH_TRAVERSAL
SKILLMD_SECURITY_CHECK_FAILED
PLUGIN_INSTALL_FAILED
PLUGIN_PERMISSION_REQUIRED
PLUGIN_AUTH_REQUIRED
PLUGIN_ENABLE_FAILED
```

Do not silently fall back from a plugin to only its first SKILL.md.

If a plugin cannot be installed as a complete plugin, tell the user why.

---

# 18. Caching

Do not hit SkillMD on every UI redraw.

Recommended:

```text
Search results:
short cache

Plugin metadata:
medium cache

Downloaded artifacts:
content-addressed cache

Installed plugins:
local/VM inventory is authoritative
```

The UI should be responsive even if SkillMD is temporarily unavailable.

Marketplace browsing can show cached results, but installation should verify/download the current selected artifact.

---

# 19. Rust HTTP implementation

Use the HTTP client already used by Supercharge instead of adding another HTTP stack unnecessarily.

Pseudo-code:

```rust
pub async fn search_skillmd(
    client: &reqwest::Client,
    query: &str,
) -> anyhow::Result<SkillMdSearchResponse> {
    let url = format!(
        "https://api.skillmd.com/v1/search?q={}",
        urlencoding::encode(query)
    );

    let response = client
        .get(url)
        .send()
        .await?
        .error_for_status()?;

    Ok(response.json().await?)
}
```

Use the repository's existing error/logging/config conventions.

Do not put SkillMD credentials in the client; public search/lookup does not require an account.

---

# 20. CLI commands

Suggested Supercharge commands:

```bash
supercharge marketplace search pdf
```

```bash
supercharge marketplace info skillmd:anthropic/pdf
```

```bash
supercharge marketplace install skillmd:anthropic/pdf
```

Plugin:

```bash
supercharge marketplace install plugin:skillmd/docs-writing
```

Optional:

```bash
supercharge marketplace list
supercharge marketplace installed
supercharge marketplace remove <id>
supercharge marketplace update <id>
```

The CLI should use the same installer/backend as the GUI.

Do not implement separate installation logic for CLI and GUI.

---

# 21. API exposed by Supercharge

If exposing marketplace functionality through the Supercharge daemon API, use something like:

```http
GET /marketplace/search?q=pdf&source=skillmd
```

```http
GET /marketplace/items/<encoded-id>
```

```http
POST /marketplace/install
```

Example conceptual body:

```json
{
  "source": "skillmd",
  "id": "plugin:skillmd/docs-writing",
  "target": "vm",
  "bot_id": "..."
}
```

The server must validate ownership/authorization before performing an install.

The frontend should never directly control the Plugin API service.

---

# 22. Installation state

Persist:

```text
source
marketplace_id
plugin_id/skill_id
version
installed_at
artifact_digest
enabled
trusted
target_vm
target_bot
configuration_state
```

For VM installs, VM inventory remains authoritative.

The existing Supercharge marketplace handoff already describes joining installed VM inventory with catalog metadata to build "My Apps".

---

# 23. Plugin update strategy

Do NOT automatically update plugins.

Use:

```text
installed version
        ↓
marketplace version
        ↓
"Update available"
        ↓
user selects update
        ↓
download
        ↓
verify
        ↓
install staged version
        ↓
permission/config migration if needed
        ↓
explicit enable
```

If SkillMD provides a commit/version identity, preserve it in installation metadata.

Prefer immutable artifact identity/digest over mutable URLs.

---

# 24. Important compatibility issue

SkillMD's registry contains both:

1. individual skills
2. plugins/bundles

The Supercharge marketplace must distinguish them.

Do not implement:

```text
everything = SKILL.md
```

Correct:

```text
Skill
  → install SKILL.md

Plugin
  → install complete plugin bundle
  → preserve member skills and required metadata/files
```

---

# 25. Verification requirements before merge

Before merging:

### API

- Search works.
- Lookup works.
- Invalid skill returns a clean error.
- Skill download works.
- Network failures are handled.

### Plugin

- Plugin metadata loads.
- Plugin download uses the confirmed current SkillMD download mechanism.
- Complete plugin contents are preserved.
- No plugin is reduced to a single SKILL.md.

### Security

- Archive traversal blocked.
- Absolute paths blocked.
- Symlink attacks handled.
- Untrusted plugin is disabled initially.
- No automatic script execution.
- No secrets exposed.
- MCP stdio allowlist enforced.
- Plugin API credentials stay server-side.

### Runtime

- Registry refresh works.
- New skills appear without unnecessary VM restart.
- MCP plugin tools become available through existing bridge logic.
- Existing sessions are refreshed using the existing plugin reload mechanism.

### Existing functionality

- Existing curated/pinned Plugin API catalog still works.
- Existing managed MCP installation still works.
- Existing permission approval still works.
- Existing VM isolation still works.

---

# 26. Recommended implementation order

## Phase 1 — SkillMD read-only marketplace

Implement:

```text
GET /v1/search
GET /v1/skills/<owner>/<name>
```

Build marketplace cards/detail pages.

No installation yet.

## Phase 2 — Individual skill installation

Implement:

```text
metadata
→ raw SKILL.md
→ quarantine
→ validation
→ install
→ registry refresh
```

## Phase 3 — Plugin discovery

Add SkillMD plugin listing/detail parsing.

Display:

```text
plugin name
publisher
description
skill count
skills included
safety information
```

## Phase 4 — Plugin download

Determine the current official SkillMD plugin download/bundle endpoint from its OpenAPI/site implementation.

Do not guess the endpoint.

Implement:

```text
download complete bundle
→ validate
→ quarantine
→ extract safely
→ install complete plugin
```

## Phase 5 — VM installation

Route the installation through the existing Supercharge Plugin API/controller.

## Phase 6 — Auth/permissions

Integrate:

```text
OAuth
Secrets
Permission approval
Enable
```

## Phase 7 — Live activation

Use existing:

```text
PluginRegistry
MCP refresh
skill discovery
live session refresh
```

---

# 27. What NOT to build

Do NOT:

- replace PluginRegistry
- replace Plugin API
- replace MCP bridge
- create a second independent plugin runtime
- copy every SkillMD file directly into `/var/lib/superagent/skills`
- trust SkillMD content automatically
- execute downloaded scripts during browsing
- put SkillMD/plugin credentials into the mobile app
- put Plugin API management tokens into the browser
- hard-code an undocumented SkillMD ZIP URL
- require Node.js/npm merely to browse/install SkillMD from Supercharge

The Supercharge Rust implementation should communicate with SkillMD directly over HTTPS.

---

# 28. Final target architecture

```text
                         SkillMD
                           |
                    Public Registry API
                           |
             +-------------+-------------+
             |                           |
        Search/Metadata             Artifact
             |                           |
             v                           v
      Supercharge Marketplace      Quarantine
             |                           |
             |                    Security validation
             |                           |
             +-------------+-------------+
                           |
                    Plugin Installer
                           |
              +------------+-------------+
              |                          |
          Individual Skill            Plugin
              |                          |
          SKILL.md              complete bundle
              |                          |
              +------------+-------------+
                           |
                     Plugin API
                           |
                 disabled + untrusted
                           |
                 auth / configuration
                           |
                 permission approval
                           |
                        enable
                           |
                  PluginRegistry refresh
                    /       |       \
                Skills     MCP      Hooks
                           |
                     MCP Bridge
                           |
                      Conversation
                           |
                         Agent
```

---

# 29. Acceptance criterion

The feature is complete when a user can:

1. Open Supercharge Marketplace.
2. Search for a SkillMD skill/plugin.
3. Open its detail page.
4. See publisher, description, capabilities/safety metadata and included skills where available.
5. Click Install.
6. Select a VM/bot when required.
7. Supercharge downloads the correct artifact.
8. The artifact is quarantined and validated.
9. The plugin/skill is installed through the existing Supercharge installation architecture.
10. It starts disabled/untrusted.
11. Required auth/configuration is completed securely.
12. The user explicitly approves permissions.
13. The plugin is enabled.
14. Existing PluginRegistry/MCP/skill refresh makes it available.
15. The user can attach/use it in a conversation.
16. No VM restart is required unless a specific plugin runtime genuinely requires one.
17. Existing Supercharge plugins continue working.

---

# 30. Source references

SkillMD:

- https://skillmd.com/docs
- https://skillmd.com/plugins
- https://github.com/skillmds/skillmd
- https://skillmd.com/docs/what-is-an-agent-skill
- https://skillmd.com/docs/getting-started

Supercharge:

- `PLUGIN_MARKETPLACE_HANDOFF.md`
- `.env.runtime.example`
- existing PluginRegistry/plugin discovery code
- existing Plugin API controller
- existing managed MCP installation flow

IMPORTANT: Re-check SkillMD's current OpenAPI/registry API specification before implementing any endpoint not explicitly documented above. In particular, the exact HTTP URL used to download a complete plugin bundle should be treated as an implementation detail until confirmed from SkillMD's current API/site.
