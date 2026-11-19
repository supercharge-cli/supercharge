# UI Design with UI SDK

Supercharge AI ships a built-in **UI skill** (`ui-sdk`) that teaches the agent to build interfaces from the [UI SDK](https://ui-sdk.dev/) behavior library (`@ui-sdk/*`). Instead of hand-writing keyboard handling, focus management, popups and ARIA, the agent composes ready-made behaviors such as `createPopup`, `createSelection`, `createTypeahead`, `createRovingFocus` and `createListbox`, assembles new components with `definePrimitive` and `createParts`, and renders them in React, Vue, Svelte, Solid or Web Components.

---

## The built-in UI skill

The skill is written to `~/.supercharge/builtin-skills/ui-sdk/` when Supercharge AI starts and appears in the skill list like any other bundled skill. Invoke it directly with `/ui-sdk <task>`, or let it switch on by itself.

It covers:

- choosing a shipped component for each job (Select, Combobox, Command, Dialog, Tabs, Tree, ...);
- building components the library doesn't ship from behaviors, with the spec-first checklist (focus, selection, Escape, focus recovery);
- rendering a primitive in each framework with `createParts`;
- making the interface agent-ready (`createAgent`, `data-agent-confirm`, `data-agent-secret`, `data-agent-hidden`);
- the verification loop: typecheck, build, tests, and driving the real UI.

### Automatic activation

When the session's workspace is a frontend project, Supercharge AI adds a short note to the system prompt that turns the skill on. A workspace counts as a frontend project when its `package.json` (or one in a parent folder, or in a direct subfolder, `apps/*` or `packages/*` of a monorepo) depends on one of:

| Framework | Detected from | Adapter |
|---|---|---|
| React | `react-dom`, `next`, `@remix-run/react`, `gatsby`, `react` (without `react-native`) | `@ui-sdk/react` |
| Vue | `vue`, `nuxt` | `@ui-sdk/vue` |
| Svelte | `svelte`, `@sveltejs/kit` | `@ui-sdk/svelte` |
| Solid | `solid-js`, `@solidjs/start` | `@ui-sdk/solid` |
| Web Components | `lit`, `@stencil/core`; also Angular, Preact, Astro, Qwik or plain Vite/Parcel apps | `@ui-sdk/web` |

The note names the framework and the install command for the project's own package manager, detected from the lockfile:

| Lockfile | Install command |
|---|---|
| `bun.lock` / `bun.lockb` | `bun add @ui-sdk/core @ui-sdk/<adapter>` |
| `pnpm-lock.yaml` | `pnpm add ...` |
| `yarn.lock` | `yarn add ...` |
| `package-lock.json` / none | `npm install ...` |

When there is no lockfile, the `packageManager` field in `package.json` decides, and npm is the fallback. In a monorepo the lockfile at the repository root wins.

The agent installs `@ui-sdk/core` and the adapter only when `node_modules/@ui-sdk/` is missing, and reads the docs that ship inside the installed packages (`node_modules/@ui-sdk/<framework>/docs/llms.txt`).

To turn automatic activation off:

```toml
# ~/.supercharge/config.toml
[design]
auto_skill = false
```

The skill stays available through `/ui-sdk` when activation is off.

---

## Design mode

Design mode makes the agent work as a product designer and frontend engineer for the whole session: it plans states and keyboard paths before markup, composes UI SDK components and behaviors instead of hand-rolling them, matches the project's design system, and runs a verification loop before it reports done.

| How | What it does |
|---|---|
| `/design on` / `/design off` | Turn design mode on or off. Saved as `[design] enabled` in `~/.supercharge/config.toml`, so it applies to new sessions too. |
| `/design` or `/design status` | Show whether design mode is on, the detected project (framework, package manager, verification commands) and whether UI SDK is installed. |
| `/design <what to build>` | One design task with the full design brief, without turning the mode on, e.g. `/design a country picker with type-ahead`. |
| `supercharge ui ["<what to build>"]` | Start an interactive session with design mode on for this session only. |
| `SUPERCHARGE_DESIGN_MODE=1` | Same as `supercharge ui` for any launch, including `supercharge agent stdio`. `0` forces it off. The variable wins over the saved setting. |

```toml
# ~/.supercharge/config.toml
[design]
enabled = true      # design mode in every session
auto_skill = true   # UI skill note in frontend projects (default)
```

### The verification loop: `ui_verify`

While the UI skill is active (a frontend project, or design mode) the agent gets the `ui_verify` tool. It runs the project's own checks with the project's package manager and stops at the first failure:

1. **typecheck**: the `typecheck`, `type-check`, `check-types`, `types`, `tsc` or `check` script. Without one, and when the project has a `tsconfig.json`: `vue-tsc --noEmit` for Vue, `svelte-check` for Svelte, `tsc -b` for solution-style configs, otherwise `tsc --noEmit`;
2. **build**: the `build` script;
3. **lint**: the `lint` script, only when asked.

Each result reports the command, exit code, duration and the end of its output (colour codes removed). Commands run with `CI=1`, have a 10-minute limit each (`timeout_secs`, up to 30 minutes), and are stopped together with everything they started when they time out or Supercharge exits. After it creates or changes a component the agent calls `ui_verify`, fixes the first real error, and repeats until every check passes. It never silences a check to get there.

---

## Supercharge Studio and ACP

Everything works the same in ACP agent mode (`supercharge agent stdio`, `supercharge agent serve`):

- the skill is written at startup and advertised in the session's available commands as `/ui-sdk`;
- `/design` is advertised as a built-in command (`on|off|status|<what to build>`); `/design <task>` sends the design brief as the user message, and the other forms answer with a short status message and end the turn;
- the UI skill note and the design-mode block are refreshed in the system prompt each turn;
- `ui_verify` appears in the session's tools while the UI skill note or design mode is active, and disappears when neither is;
- set `SUPERCHARGE_DESIGN_MODE=1` in the agent's environment to start every session of that process in design mode.
