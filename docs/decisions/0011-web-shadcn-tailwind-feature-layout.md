# ADR 0011 — Chat surface: shadcn/ui on Tailwind v4, kebab-case feature layout

- **Status:** accepted (2026-09-29)

## Context

[ADR 0006](0006-assistant-ui-external-store.md) fixed the runtime (Next.js and assistant-ui's
external store) but not how the UI is built. The first version was hand-written CSS with BEM class
names, PascalCase component files and a flat `src/{api,chat,components}` layout. The owner asked
for shadcn/ui, kebab-case file names and a cleaner structure.

Constraints that carry over: the design stays calm (flat surfaces, one accent, no gradients or
decorative effects), light and dark follow the system setting, and the accessibility bars stay:
axe reports no serious or critical issue and Lighthouse accessibility is at least 95, both in light
and dark, on every change.

## Decision

- **shadcn/ui on Tailwind CSS v4.** The components are generated into `src/components/ui`
  (radix base, `radix-vega` style, neutral base colour, CSS variables) and owned by this repo. The
  CLI is pinned (`shadcn@4.21.0`) and `components.json` is committed. Generated files are
  formatted by Biome and may be edited; an edit is a deliberate local change, not a rule exception.
- **Design tokens are the palette the app already shipped with**, mapped onto shadcn's token names
  in `src/app/globals.css` (plus `--success` and `--warning`). Dark mode is
  `@media (prefers-color-scheme: dark)`, with the `dark:` variant redefined to the same media
  query: no toggle, no `next-themes`, no `.dark` class. The system font stack stays, so nothing is
  downloaded at build or run time.
- **Native selects for the agent and release pickers** (`NativeSelect`, not Radix `Select`): the
  best control on a phone, and `<optgroup>` groups the revisions.
- **assistant-ui registry components as the starting point, then pruned.** `thread`, `thread-list`
  and `markdown-text` come from the `@assistant-ui` shadcn registry (`components.json` →
  `registries`). What the event log has no handlers for is removed: voice, attachments,
  suggestions, feedback, reload, copy, edit, branches, archive, delete, rename and search. Status
  lines, artifacts and errors are `data-*` parts rendered by `makeAssistantDataUI` renderers
  (`features/chat/components/data-uis.tsx`) through `part.dataRendererUI`.
- **Phones get a sheet, not a disclosure.** Below 768px a "Threads" button opens the thread list
  in a left `Sheet` (titled, focus returns to the button, closes when a thread is picked). Above it
  the list is a fixed column. Only one "Threads" navigation is in the accessibility tree at a time.
- **Layout: feature-oriented, kebab-case everywhere.**

  ```
  src/app/                       routes only (thin pages)
  src/components/ui/             shadcn primitives (generated)
  src/components/assistant-ui/   registry items, pruned (thread.aui, thread-list.aui, markdown-text)
  src/features/{chat,threads,agents}/{components,hooks,lib}
  src/lib/                       api client, generated types (gitignored), cn()
  ```

  Tests are colocated with the code (`*.test.ts`, `*.dom.test.tsx`); end-to-end tests stay in
  `e2e/` and `e2e-system/` and reach the DOM only through the locators in `e2e/helpers.ts` (roles
  and accessible names first).
- **Biome enforces the file names** (`style/useFilenamingConvention`, kebab-case): a PascalCase
  file fails `pnpm check` and therefore CI.

## Alternatives rejected

- **Radix `Select` for the pickers.** Worse on phones, no `<optgroup>`, and every
  `selectOption` test would need a rewrite.
- **A `.dark` class with a toggle (`next-themes`).** A setting nobody asked for, and the
  accessibility suite emulates the system colour scheme.
- **Keeping the hand-written CSS.** BEM rules cannot share the component library the OpenUI work
  builds on.
- **Installing the registry `thread` unpruned.** It ships controls that would do nothing here.

## Consequences

- New UI is composed from `src/components/ui` primitives and the tokens, never from new global CSS.
- A change to a registry-derived file is a change to our copy; upstream updates are merged by
  hand, so the diff is small on purpose.
- Colour is never the only carrier of state: every badge and status has a text label.

## Verified

- *Verified 2026-09-29* against the `shadcn@4.21.0` CLI (`init -b radix -p vega`, `add`), Tailwind
  4.3.3 and the `https://r.assistant-ui.com/styles/{style}/{name}.json` registry: the flow works as
  described. Notes: `init` requires Tailwind and PostCSS to be installed first, and the CLI writes
  `from "cn"` imports (the `cn` package is a transitive dependency of `shadcn`); they are rewritten
  to `@/lib/utils`.
- *Verified 2026-09-29*: Biome 2.5.14 accepts `css.parser.tailwindDirectives` and
  `useFilenamingConvention` with `filenameCases: ["kebab-case"]`; Next's `[id]` segment and
  `thread.aui.tsx` style names pass.
