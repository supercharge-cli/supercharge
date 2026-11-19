---
name: ui-sdk
description: Build and change user interfaces with the UI SDK behavior library (@ui-sdk/*) in React, Vue, Svelte, Solid or Web Components. Use for any frontend UI work - pages, forms, pickers, menus, dialogs, comboboxes, trees, grids, editors - and for new components the library doesn't ship, composed from behaviors like createPopup, createSelection, createTypeahead, createRovingFocus and createListbox with definePrimitive and createParts. Also covers making the UI agent-ready and verifying it with typecheck, build and the Supercharge UI tools.
---

# Supercharge UI skill: build interfaces from UI SDK behaviors

UI SDK (`@ui-sdk/*`) is a library of UI *behaviors*: keyboard navigation, focus, selection, popups, dismissal, drag and drop, and the ARIA each one needs. Its components are assembled from those behaviors, ship no styles of their own, and exist for React, Vue, Svelte, Solid and Web Components. When a component is missing you assemble it from the same behaviors, so it inherits the same keyboard, focus, right-to-left and server-rendering support.

Think in behaviors, not widgets. A Select is a popup (`createPopup`) + choosing from a list (`createSelection`) + jump-by-typing (`createTypeahead`). Rearranged, those same pieces give you an autocomplete, a combobox, a teammate picker or something new.

## 1. Set up

1. Detect the framework from `package.json` (`react`/`next` -> `@ui-sdk/react`, `vue`/`nuxt` -> `@ui-sdk/vue`, `svelte`/`@sveltejs/kit` -> `@ui-sdk/svelte`, `solid-js` -> `@ui-sdk/solid`, `lit`, plain HTML or anything else -> `@ui-sdk/web`).
2. If `node_modules/@ui-sdk/` is missing, install `@ui-sdk/core` plus the framework adapter **with the project's own package manager**, picked from the lockfile: `bun.lock`/`bun.lockb` -> `bun add`, `pnpm-lock.yaml` -> `pnpm add`, `yarn.lock` -> `yarn add`, otherwise `npm install`. Keep every `@ui-sdk/*` package on the same version. Nuxt 4+: also add `"@ui-sdk/vue"` to `modules` in `nuxt.config.ts`.
3. Read the docs that ship with the installed version, not memory: `node_modules/@ui-sdk/<framework>/docs/llms.txt` is the index (`<framework>` is `react`, `vue`, `svelte`, `solid` or `web`). Open only the pages you need, such as `docs/components/select.md`. `docs/core.md` is very large: search it by heading. The `.d.ts` files are the API reference; never read compiled `dist/*.js`.
4. A page with no build step can load the Web Components bundle from `https://ui-sdk.dev/web.js` and `https://ui-sdk.dev/web.css`; its single-file docs are at `https://ui-sdk.dev/web.md`.

## 2. Pick the path

- **Assemble from shipped components** (settings page, form, command palette, data entry): read [references/components.md](references/components.md) and use the table there to choose.
- **Build a component or behavior the library lacks** (stepper, splitter, transfer list, emoji grid, user picker): read [references/behaviors.md](references/behaviors.md), then [references/frameworks.md](references/frameworks.md) to render it.

Every path then ends with [references/agent-ready.md](references/agent-ready.md) and [references/verify.md](references/verify.md).

## 3. Rules that apply everywhere

- **Import from per-component entries**, like `@ui-sdk/react/select`, never from a package root for components and never from `@ui-sdk/core/internal/*`.
- **Root and parts.** Every component is a root with named parts nested as its docs' Composition section shows. Swap a part's element with `render`, never an `as` prop.
- **Uncontrolled or controlled.** `defaultValue` to start, or `value` + `onValueChange(value, details)` (Vue `v-model`, Svelte `bind:value`, Web Components properties + events). `details` carries `reason`, `source` and `cancel()`.
- **Every control has an accessible name**: its label part (`FieldLabel`, `SelectLabel`, `DialogTitle`) or `aria-label`. Screen readers and agents find controls by name.
- **Style from state attributes**, not inline styles: target `data-slot` plus state attributes such as `data-open`, `data-pressed`, `data-highlighted`, and the CSS variables each component documents. Put `@layer ui-sdk;` at the top of the global stylesheet (above `@import "tailwindcss"` when present). The optional default theme is `@ui-sdk/core/web.css`.
- **Contrast**: text 4.5:1 against its background; control edges, focus rings and state fills 3:1. jsdom can't see color, so check in a real browser.
- **Key lists by a stable value or id**, never by array index.
- **Right-to-left needs no provider**; components read direction from the page.
- **Don't hand-roll shipped behavior.** A `div` with `role="listbox" | "menu" | "dialog" | "tablist" | "switch" | "slider" | "radiogroup" | "toolbar" | "tree"` next to an installed `@ui-sdk` package is almost always a bug: use the component or the behavior.
- **Comments are rare**: only browser quirks, accessibility-pattern rules or real trade-offs, one or two lines.

## 4. Done means verified

1. Typecheck and build pass (see [references/verify.md](references/verify.md); in Supercharge, call `ui_verify` when it is available).
2. Tests exist and pass, including the agent test from agent-ready.md.
3. When a dev server and the `ui_browser` tool are available, open the app, read the accessibility snapshot, and drive the new controls with clicks and keys.
4. Report each command with its exit code, anything you could not check, and any gap you found in the docs or types.
