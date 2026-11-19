# Assemble an interface from shipped components

Open `node_modules/@ui-sdk/<framework>/docs/llms.txt`, then the page for every component you pick (`docs/components/<name>.md`). Its *Composition* section is the source of truth for part names and nesting; its API section lists props, state attributes, CSS variables and change reasons.

## Choose by what the person is doing

| Job | Component |
| --- | --- |
| Run an action | Button |
| Turn a setting on or off | Switch |
| Press-and-stay button, or a group of them | Toggle |
| Independent yes/no options | Checkbox |
| One of a few visible options | Radio, or Segmented Control for a compact row |
| One or several from a popup list | Select |
| From an always-visible list | Listbox |
| Search a list, then choose | Combobox |
| Free text with suggestions | Autocomplete |
| Search and run commands | Command |
| Mention or tag with `@`, `/`, `#` while typing | Trigger Menu |
| Text mixed with tokens (people, files, tags) | Token Input |
| Text | Input, or Textarea |
| Number (money: `step={0.01}`) | Number Input |
| Password / one-time code | Password Input / OTP Input |
| Date, time | Date Input, Date Picker, Calendar, Time Input |
| Value or range by dragging | Slider |
| Score | Rating |
| Color | Color Picker |
| Files (choose, drop, paste) | File Input |
| Crop an image | Image Cropper |
| Label + description + error for a control | Field with its Label |
| Confirm a destructive action | Alert Dialog (`AlertDialogClose value="confirm"` is the yes) |
| Focused task over the page | Dialog, or Drawer from an edge |
| Content anchored beside a trigger | Popover |
| Actions from a button or right click | Menu; app menu bar: Menubar |
| Buttons and toggles with one tab stop | Toolbar |
| Short hint or name | Tooltip; link preview: Hover Card |
| Site navigation with panels | Navigation Menu |
| Switch panels | Tabs |
| Show/hide sections | Accordion, or Collapsible for one |
| Hierarchy | Tree |
| Time-axis bars (traces, timings) | Waterfall |
| Paging | Pagination; slides: Carousel |
| Resizable panels / styled scrolling | Resizable / Scroll Area |
| Notifications, progress, measurements | Toast, Progress, Meter |
| User image with fallback, divider | Avatar, Separator |
| Grid of choices (seats, emoji, swatches) | your own primitive on `createGridFocus` or `createRovingFocus` |
| Drag to reorder or move between lists | your own primitive on `createReorder` / `createTransfer` |

Nothing fits? Build only that piece from behaviors ([behaviors.md](behaviors.md)); never fake it with ARIA in plain markup.

## Wiring

- Nest parts exactly as Composition shows. Some roots (Popover) render nothing and only share state; others (Tabs) render an element.
- `render` swaps a part's element (a menu item that is a link). A custom element passed to `render` must forward props and ref.
- List components take `items` as strings or `{ value, label }` objects.
- Change a component from outside its tree with its controller (see the Controllers docs page).

## State per framework

| Framework | Controlled value | Change callback |
| --- | --- | --- |
| React | `value={v} onValueChange={setV}` | `onValueChange(value, details)` |
| Vue | `v-model="v"` (`v-model:open` for other keys) | `@value-change="(value, details) => ..."` |
| Svelte | `bind:value` on a `$state` | `onValueChange(value, details)` |
| Solid | `value={v()} onValueChange={setV}` | `onValueChange(value, details)` |
| Web Components | `el.value = ...` | `valuechange` event (`event.detail`), cancel with `event.preventDefault()` |

`details.reason` is why (`item-press`, `escape-key`, `outside-press`, ...), `details.source` is how (`keyboard`, `mouse`, `agent`, ...), `details.cancel()` refuses it. Single Select, Tabs and Radio report `null` for "nothing"; multiple Select and Combobox use arrays.

## Forms

- A control with a `name` submits with its form (Select and Combobox too).
- Wrap controls in `Field` with `FieldLabel`, `FieldDescription`, `FieldError`. Put `Input`/`Textarea`/`NumberInput`/`PasswordInput` inside for text; `FieldControl` is a bare native input that agents cannot see, so avoid it for text.
- `required`, `disabled`, `readOnly`, `form` behave like native attributes. Server errors: `invalid` on the Field plus the message in `FieldError`.

## Styling

- Every part carries `data-slot` (`switch` on a root, `switch-thumb` on a part). Read the rendered DOM before writing selectors.
- State attributes: `data-open`, `data-pressed`, `data-disabled`, `data-highlighted`, `data-side="top"`, ... Tailwind: `data-pressed:bg-neutral-100`; CSS: `[data-slot="select-trigger"][data-open]`.
- Popups get `data-starting-style` on enter and `data-ending-style` on exit and stay mounted until the exit finishes. They expose `--anchor-width`, `--available-height`, `--transform-origin`.
- With the default theme (`@ui-sdk/core/web.css`), read its styles page first: it draws icons and sets sizes you can't see in the markup.

## Framework notes

- **React**: shipped components already declare `"use client"`; your file needs it when it holds state, effects, handlers, or calls `createParts`.
- **Vue**: kebab-case props and events in templates (`default-value`, `@open-change`).
- **Svelte**: `bind:` for parent-owned values; share state via `setContext`/`getContext` from a parent; never module-level `$state` (it leaks across server requests); type props with `interface Props` and `$props()`.
- **Solid**: keep reads reactive (read signals in JSX or getters). `@ui-sdk/solid` needs Solid 2. With Vite, add `optimizeDeps: { exclude: ["@ui-sdk/core"] }`.
- **Web Components**: `import "@ui-sdk/web/<name>"` defines the tags (`<ui-select>`, ...). Many parts wrap native markup you write: `<ui-button><button>Save</button></ui-button>`, `<ui-field><ui-field-label>Name</ui-field-label><ui-input><input /></ui-input></ui-field>`. Wait for `customElements.whenDefined(...)` before reading properties. Events are lowercase (`valuechange`, `openchange`) and do not bubble.

## Example (React settings form)

```tsx
"use client"

import { AlertDialog, AlertDialogClose, AlertDialogContent, AlertDialogTitle, AlertDialogTrigger } from "@ui-sdk/react/alert-dialog"
import { Field, FieldLabel } from "@ui-sdk/react/field"
import { Input } from "@ui-sdk/react/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@ui-sdk/react/select"
import { Switch, SwitchThumb } from "@ui-sdk/react/switch"

export function ProjectSettings() {
  return (
    <>
      <form>
        <Field name="name">
          <FieldLabel>Project name</FieldLabel>
          <Input defaultValue="Apollo" required />
        </Field>
        <Field>
          <FieldLabel>Visibility</FieldLabel>
          <Select name="visibility" defaultValue="private">
            <SelectTrigger><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="private">Private</SelectItem>
              <SelectItem value="public">Public</SelectItem>
            </SelectContent>
          </Select>
        </Field>
        <Field>
          <Switch name="digest" defaultChecked><SwitchThumb /></Switch>
          <FieldLabel>Weekly digest</FieldLabel>
        </Field>
      </form>
      <AlertDialog>
        <AlertDialogTrigger>Delete project</AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogTitle>Delete Apollo?</AlertDialogTitle>
          <AlertDialogClose>Cancel</AlertDialogClose>
          <AlertDialogClose value="confirm" data-agent-confirm="Deletes the project and all its data">
            Delete forever
          </AlertDialogClose>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
```

Always re-check part names against the installed component pages.
