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

## Supercharge Studio and ACP

Nothing extra is needed in ACP agent mode: the skill is written at startup by `supercharge agent stdio` and `supercharge agent serve`, is advertised in the session's available commands as `/ui-sdk`, and the automatic system-prompt note is applied per turn in every ACP session.
