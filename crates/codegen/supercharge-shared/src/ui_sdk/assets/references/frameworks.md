# Render a primitive in each framework

`createParts(primitive, { slot: "tag", ... })` turns a primitive into framework components. `Root` creates the instance and shares it with the parts inside; each mapped slot becomes a component named in PascalCase (`star` -> `Star`, `tab-icon` -> `TabIcon`). Leave out `root` and `Root` shares the instance without rendering an element. The render file never adds key handlers, focus moves, ARIA or `data-slot`: that is the primitive's job. Write a custom part with `usePart` (or a root with `usePrimitive`) only when it must render something of its own, such as a list from data.

All examples render the same `rating` primitive with `root` and `star` slots.

## React (`@ui-sdk/react/primitive`)

```tsx
"use client"
import { createParts } from "@ui-sdk/react/primitive"
import { rating } from "./rating"

export const { Root: RatingRoot, Star: RatingStar } = createParts(rating, { root: "div", star: "button" })

export function Example() {
  return (
    <RatingRoot defaultValue={4} aria-label="Rating">
      {["1", "2", "3", "4", "5"].map((v) => (
        <RatingStar key={v} value={v} aria-label={`${v} stars`} className="opacity-40 data-filled:opacity-100">★</RatingStar>
      ))}
    </RatingRoot>
  )
}
```

The file that calls `createParts` starts with `"use client"`.

## Vue (`@ui-sdk/vue/primitive`)

```ts
// rating-parts.ts
import { createParts } from "@ui-sdk/vue/primitive"
import { rating } from "./rating"
export const { Root: RatingRoot, Star: RatingStar } = createParts(rating, { root: "div", star: "button" })
```

```vue
<script setup lang="ts">
import { RatingRoot, RatingStar } from "./rating-parts"
</script>
<template>
  <RatingRoot :default-value="4" aria-label="Rating">
    <RatingStar v-for="star in 5" :key="star" :value="String(star)" :aria-label="`${star} stars`">★</RatingStar>
  </RatingRoot>
</template>
```

Generated parts accept the same default and `#render` slots as catalog components.

## Svelte (`@ui-sdk/svelte/primitive`)

```svelte
<script lang="ts">
  import { RatingRoot, RatingStar } from "./rating-parts" // createParts from "@ui-sdk/svelte/primitive"
</script>

<RatingRoot defaultValue={4} aria-label="Rating">
  {#each [1, 2, 3, 4, 5] as star (star)}
    <RatingStar value={String(star)} aria-label={`${star} stars`}>★</RatingStar>
  {/each}
</RatingRoot>
```

Parts accept `children` and `render` snippets.

## Solid (`@ui-sdk/solid/primitive`)

```tsx
import { For } from "solid-js"
import { RatingRoot, RatingStar } from "./rating-parts" // createParts from "@ui-sdk/solid/primitive"

export function Example() {
  return (
    <RatingRoot defaultValue={4} aria-label="Rating">
      <For each={[1, 2, 3, 4, 5]}>
        {(star) => <RatingStar value={String(star)} aria-label={`${star} stars`}>★</RatingStar>}
      </For>
    </RatingRoot>
  )
}
```

## Web Components (`@ui-sdk/web`)

There is no `primitive` entry: `createParts` comes from `@ui-sdk/web` and registers one custom element per mapped slot.

```ts
import { createParts, wrappedButton } from "@ui-sdk/web"
import { rating } from "./rating"

createParts(rating, {
  root: "my-rating",
  star: { tag: "my-rating-star", target: wrappedButton, display: "contents" },
})
```

```html
<my-rating default-value="4" aria-label="Rating">
  <my-rating-star value="1"><button aria-label="1 star">★</button></my-rating-star>
  <my-rating-star value="2"><button aria-label="2 stars">★</button></my-rating-star>
</my-rating>
```

`wrappedButton` puts the slot's behavior on the native button inside. `onValueChange` becomes a non-bubbling `valuechange` event on the root; `event.preventDefault()` cancels. Load the module from the app's browser entry; nothing registers on the server.

## Adapters must match

Every adapter is its own package at the same version as `@ui-sdk/core`. Install the one you need with the project's package manager and say so in the report.
