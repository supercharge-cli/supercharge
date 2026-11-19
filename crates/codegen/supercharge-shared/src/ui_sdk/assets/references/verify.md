# Verify before you call it done

## 1. Typecheck and build (Supercharge: `ui_verify`)

When the Supercharge `ui_verify` tool is available, call it after every component change. It detects the project's package manager and runs its typecheck and build (and lint when asked), returning each command, its exit code and the tail of its output. Fix the first real error, then call it again; repeat until everything passes. Never declare a UI task done while `ui_verify` fails, and never "fix" a failure by deleting a check, loosening `tsconfig`, or adding `// @ts-ignore`.

Without the tool, run the same commands yourself:

- the `typecheck`/`type-check`/`check-types`/`check` script when `package.json` has one;
- otherwise TypeScript directly: `tsc --noEmit` (a root `tsconfig.json` with `"files": []` and `references` checks nothing - use `tsc -b` or `-p <app config>`), `vue-tsc --noEmit` for Vue, `svelte-check` for Svelte;
- the `build` script.

Run binaries through the project's package manager (`pnpm exec`, `bunx`, `yarn run`, `npx`).

## 2. Tests

- Use the project's runner (usually Vitest) with jsdom and Testing Library. jsdom lacks some layout and pointer APIs; stub them before anything opens a popup (`ResizeObserver`, `Element.prototype.scrollIntoView`, `hasPointerCapture`/`setPointerCapture`, `matchMedia`).
- Test behavior by role and name: keyboard paths (Arrow keys, Home/End, Enter, Space, Escape, Tab), focus placement, ARIA states, change reasons, and the agent test.
- For a new component, break it on purpose (remove a key handler, an ARIA attribute) and confirm a test fails, then restore it.
- If a test reveals a bug in code you were only asked to test, mark it `test.fails` with a one-line reason and report it.

## 3. Drive the real UI (Supercharge: `ui_browser`)

When the Supercharge `ui_browser` tool is available, start the dev server in the background (for example `pnpm dev`), then:

1. `ui_browser` `open` with the local URL.
2. `snapshot` to read the accessibility tree: every control should appear with a role and a sensible name. Unnamed controls are bugs.
3. Work the controls the way a person would: `click` by ref or by role + name, `type` text, `press` keys (`ArrowDown`, `Enter`, `Escape`, `Tab`, `Shift+Tab`, `Control+a`).
4. `snapshot` again and confirm states changed (expanded, selected, checked, focused, values).
5. `close` when finished, and stop the dev server you started.

The tool uses an installed Chrome, Chromium or Edge in headless mode. When none is installed it says so; fall back to the unit and agent tests and mention it in the report.

## 4. Report

List each command with its exit code, what you verified in the browser, anything you could not check, and any gap you found in the library's docs or types.
