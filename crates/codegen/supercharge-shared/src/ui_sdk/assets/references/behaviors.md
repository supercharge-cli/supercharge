# Build a component from behaviors

A component the library doesn't ship is **one primitive** (`definePrimitive` from `@ui-sdk/core`) plus **one small render file per framework** (`createParts`). The primitive owns state, keys, focus and ARIA; the render file only maps slots to elements.

Before writing code, read in the installed package: `docs/core.md` (search by heading: "Build a Component", "Compose Behaviors", "Popups and Layers", and the Behaviors page for each block you use), `docs/rendering.md` for your framework, and the `.d.ts` JSDoc under `node_modules/@ui-sdk/core/dist/`. List the entries in the adapter's `package.json`; if one already covers part of the job, compose it instead of rewriting it.

## The behavior blocks

| Need | Block |
| --- | --- |
| Something that opens next to a trigger, stays on screen, dismisses on Escape/outside press | `createPopup` with `popupState`, `popupTrigger`, `popupContent` (presets such as `popoverPreset`) |
| Pick one or many, Shift/Ctrl ranges like a file manager | `createSelection` (`choose`, `toggle`, `extend`) |
| Type a few letters to jump to an item | `createTypeahead` (returns the search fn with `reset()` and `active()`) |
| Arrow keys through a group with a single Tab stop | `createRovingFocus` (custom `move` for grids, trees, RTL) |
| Highlight that moves through options and filters while typing; focus stays in an input (`aria-activedescendant`) | `createListbox` (`focus: "virtual"`, `query`) |
| Cells by row and column with equal columns (calendar) | `createGridFocus` |
| Spreadsheet block selection, copy/paste | `createCellRange`, `cellsToText`, `textToCells` |
| Reorder by pointer and keyboard | `createReorder`; between lists: `createTransfer` |
| Resize boxes from handles | `createResize` |
| Pan and zoom a canvas | `createViewport` |
| Render only visible rows | `createVirtualizer` |
| ArrowUp/ArrowDown through earlier inputs | `createInputHistory` |
| Keep focus when the focused part disappears | `createFocusRecovery` |
| Report value changes with a reason | `createChange` |
| Screen-reader messages | `announce` |
| Scroll-to-pick wheels | `bindScrollPicker` |
| A custom drag | `bindDrag` |
| Helpers | `selects`, `elements`, `isRTL`, `idPart`, `createTreeCollection`, `@ui-sdk/core/text-match`, `@ui-sdk/core/date` |

The Core docs' "All Behaviors" tables map more jobs to blocks; there are 50+.

## Write the spec first

Pick the WAI-ARIA Authoring Practices pattern and write down roles, states and keys. Decide in writing:

- where focus lands on open and on Tab, and whether the group is one Tab stop;
- whether selection follows focus (radio group, single listbox) or Space selects (multi-select, grids);
- who owns Escape (inside a popup, the popup closes);
- where focus goes when the focused part is removed (`createFocusRecovery`);
- `aria-disabled` (keeps focus) vs `disabled` (drops focus) for controls that temporarily can't act;
- which api methods are actions a person takes (they become `agent.actions`).

## Anatomy of a primitive

```ts
import { createChange, createListbox, createPopup, createSelection, definePrimitive, popoverPreset, popupContent, popupTrigger } from "@ui-sdk/core"

export const userPicker = definePrimitive({
  name: "user-picker",
  api: (context) => {
    const change = createChange(context)
    const popup = createPopup(context, { preset: popoverPreset })
    const listbox = createListbox(context, { key: "highlighted" })
    const selection = createSelection(context, {
      key: "value",
      set: (value, details) => change("value", value, details),
    })
    return { ...popup, listbox, selection }
  },
  slots: (slot) => ({
    trigger: popupTrigger(slot),
    content: popupContent(slot),
    option: slot({
      role: "option",
      props: { value: "string" },
      bind: (el, part) => part.api.listbox.bind(el, part),
    }),
  }),
  agent: { actions: ["setValue"] },
})
```

(Check exact import locations and option names against the installed `.d.ts`; the shape above is the pattern.)

- **Options**: controlled keys as `<key>`, `default<Key>`, `on<Key>Change(value, details)`.
- **Schema**: name every option and every slot prop with its kind (`"string"`, `"number"`, `"boolean"`, `"list"`, `"value"`, `"callback"`, `"action"`, `"id"`, `"key"`, or `{ kind, from: "item" }`). TypeScript enforces completeness.
- **State**: `state` gives defaults; `sync` maps controlled options into state (unsynced controlled options are silently ignored).
- **Changes**: every user-visible change goes through `createChange(context)` with a reason, preferring the shared `ReasonVocabulary`; declare new reasons in the `on<Key>Change` type.
- **Slots**: `role` on the slot (TypeScript then demands `focus`, `press` or `keys` as the role requires); `attrs` for ARIA and `data-*` (pure, server-safe); `press` for activation (clicks only on non-button parts, so handle Space/Enter in `on.keydown` for a `div`); `on` for other events; `bind` for DOM work, returning a cleanup. Every element gets `data-slot` from its slot; wrappers get their own slot.
- **Agents**: end with `agent: { actions: [...] }` after `slots`, plus `closed`, `items`, `reveals`, `shown` or `secret` when they apply (see agent-ready.md).
- **Strict tests**: set `globalThis.__UI_SDK_STRICT__ = true` in tests so development warnings fail.

## Patterns

- **Grid arrows**: `createRovingFocus` with a `move(event, { from, rtl })` that finds the next cell by id (not index) and returns `from.key` at the edges so the arrow is handled without scrolling the page.
- **Search box over options**: input slot `role: "combobox"` with `aria-activedescendant: api.listbox.activeId(state)`; option slots `focus: "active-descendant"`, `present: (s, part) => part.api.listbox.shows(part)`, `pointerdown` prevented so focus stays in the input, `pointermove` highlights.
- **Typeahead in a roving group**: only intercept Space when `typeahead.active()`; otherwise let Space toggle.
- **Derived state** depending on both options and state: recompute in a `store.subscribe` that compares references, and store the result.
- **Two lists side by side**: one `createRovingFocus` group per list, so one option slot per side built by a shared function.
- **Stepper**: buttons with one Tab stop and `aria-current="step"` on the current step; not radios or tabs.

## A new reusable behavior

When an interaction repeats across components (press-and-hold repeat, drag-to-select), write `create<Thing>(context, options)` returning methods and slot pieces, binding through a slot's `bind` with cleanup, no framework code. Test it on its own.

## Compose a shipped component inside yours

- Render theirs inside yours and control it from your state (your primitive owns `open`; theirs gets `open={state.open}` and forwards `onOpenChange` details into your `change`).
- Or wrap theirs and add behavior from outside through its public controller (`createInputController()`, or `.api` on the web element). Never bind your slot to an element theirs already binds.

Then render it: [frameworks.md](frameworks.md).
