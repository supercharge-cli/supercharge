# Make the interface agent-ready

An app built with UI SDK can let an agent read its components and call their actions directly through `createAgent`, instead of clicking pixels. The full docs are `docs/agents.md` in each adapter package.

## Checklist

1. **Every control has a unique, human name** (label part or `aria-label`). Labels match by containment, so "Tip" and "Custom tip" collide: give each control in a row its own verb ("Open Water the plants"). Items and tabs are named by their visible text. Give a shared Tooltip an `aria-label`, an Avatar `role="img"` + `aria-label`, a multi-section Accordion an `aria-label`. A Dialog with several triggers needs `keepMounted` on its content so its title names it. Don't share one Menu between rows; give each row its own trigger name ("Actions for Jane Doe").
2. **Every control an agent should use is a UI SDK part.** Plain `<button>`, bare `<input>`/`<textarea>`/`<select>` and `FieldControl` are invisible to `snapshot()`. Use `Button`, `Input`, `Textarea`, `NumberInput`, `Select` inside a `Field` (in Web Components: `<ui-button><button>...`, `<ui-input><input /></ui-input>`).
3. **State flows through change callbacks**, never DOM listeners: agent changes call the component API, so `onClick`/`onKeyDown` checks never see them. `details.source === "agent"` identifies them; `details.cancel()` refuses. What a popup shows comes from its payload or app state, not a trigger's click handler. `minLength` doesn't stop agent values; use `pattern` or your own validation.
4. **Mark what needs a person** - exactly three attributes:
   - `data-agent-confirm="<what it does>"` on anything an agent must not do alone (delete, pay, send). Put it on the confirming button (`AlertDialogClose value="confirm"`), not the trigger. Make that button's text differ from the trigger's ("Delete forever" vs "Delete project").
   - `data-agent-secret` on a field an agent may fill but never read (password, card, one-time code already are).
   - `data-agent-hidden` on regions an agent must not see (recovery codes, empty toast regions).
5. **Custom primitives declare actions**: `agent: { actions: ["rate"] }`; add `closed: true` when the value is always one of the items, `items: (api) => [...]` when items aren't in state, `reveals: true` for tabs/accordion-like hiding, `shown: (state) => bool`, `secret: true`.

## Connect the bridge (browser only)

```ts
import { agentTools, createAgent, registerTools } from "@ui-sdk/react/agent" // or vue/svelte/solid/web

export const agent = createAgent({
  allow: (step, { signal }) => (step.confirm ? askThePerson(step.confirm, { signal }) : true),
})
```

- `agent.snapshot()` lists components, their state and actions; `agent.act({ steps })` runs steps; `agent.query({ text, within, kind })` finds components and list items.
- Your own model: `agentTools` exposes the three tools' names, descriptions and JSON Schemas.
- An in-browser agent: `registerTools(agent, { allow })` (no-op without `document.modelContext`). Ask the person in one place only.
- With server rendering, create the bridge in client-only code.

## The agent test (write it for every interface)

```tsx
import { render, screen } from "@testing-library/react"
import { createAgent } from "@ui-sdk/react/agent"
import { expect, test } from "vitest"
import { ProjectSettings } from "./project-settings"

test("an agent can find and change every setting", async () => {
  const agent = createAgent()
  render(<ProjectSettings />)
  const entries = agent.snapshot().filter((e) => !e.offPage && e.component !== "more")
  for (const e of entries) expect(e.label, `${e.component} ${e.ref} has no name`).toBeTruthy()
  const labels = entries.map((e) => e.label)
  for (const label of ["Project name", "Visibility", "Weekly digest"]) expect(labels).toContain(label)
  const result = await agent.act({
    steps: [
      { ref: "Project name", action: "setValue", args: ["Atlas"] },
      { ref: "Visibility", action: "setValue", args: ["Public"] },
    ],
  })
  expect(result.ok, result.error?.message).toBe(true)
  expect(screen.getByLabelText<HTMLInputElement>("Project name").value).toBe("Atlas")
})
```

One bridge per test file; unmount between tests (`afterEach(cleanup)` unless Vitest `globals: true`). For a `data-agent-confirm` control, flip an `allow` flag and assert the step is refused, then runs. `act` can be `ok` while a callback cancelled the value: check the DOM or `result.values`.
