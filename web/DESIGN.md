# Chat surface: design brief

The owner, after trying the previous version: "It should feel like a classical chat, not a machine
to machine chat interface", "It should look like a gemini chat, with custom components … very cute
and simple", with the agent's steps always visible. This brief is the contract the components in
`src/` follow; the screenshots in `e2e/__screens__/` (`pnpm screens`) show the result.

## References

Studied on [Refero](https://refero.design) on 2026-09-30, full screen images viewed. We take the
structure, not the branding.

| Reference | What we took |
|---|---|
| Gemini, home ([965e3a7f](https://refero.design/pages/965e3a7f-d7dc-45ae-8cc1-12025b10b1fd), [6994cae8](https://refero.design/pages/6994cae8-03f9-4ef0-9fb5-95a45469cf52)) | Quiet left sidebar (New chat, Recent), a centered greeting over one pill-shaped composer, lots of air |
| Gemini, conversation ([dc0889ae](https://refero.design/pages/dc0889ae-1e9f-49de-8465-5d810e0d6f18)) | The user's words in a soft grey bubble on the right; the answer as plain prose under a small label, no bubble; a floating rounded composer with a round send button |
| ChatGPT, conversation ([2ed2ffc3](https://refero.design/pages/2ed2ffc3-95b2-417c-a3fa-df50f4cb22e4)) | Centered reading column around 760 px; title in a minimal top bar with the actions on the right; a disclaimer line under the composer |
| Claude ([f42e56ca](https://refero.design/pages/f42e56ca-6f9d-45b8-b501-ce30add0f259)) | "Reply to Claude…" placeholder for the next turn; the model named inside the composer |
| Meta AI ([6c1e2ac6](https://refero.design/pages/6c1e2ac6-e65a-4290-8ae5-81ad61ea200a)) | The assistant's mark once above its turn, then prose; generous spacing between turns |
| Copilot ([d81060e5](https://refero.design/pages/d81060e5-9955-4fb2-a8d6-b4ccaa97c312)), Grok ([0ac86436](https://refero.design/pages/0ac86436-063a-45a1-a04d-81042b245675)) | A composer with a soft shadow and chips inside it; a two-line greeting (statement, then a muted question) |
| Cursor agents (flow [9922](https://refero.design/flows/9922): [16261b66](https://refero.design/pages/16261b66-bd9b-4403-94da-7d88c4c15157), [8b09e132](https://refero.design/pages/8b09e132-8a3e-4c4b-b8cf-617b51ba3129), [d7a49941](https://refero.design/pages/d7a49941-aef5-4f63-8d48-a6e66930e104)) | An agent's steps as quiet single lines ("Completing setup", commands in monospace in a light box) between its messages; the follow-up composer names the model |
| Bard, dark ([195db751](https://refero.design/pages/195db751-c533-4953-9247-824167e79f03)) | Dark mode: near-black canvas, the sidebar one step lighter, the sparkle as the assistant's avatar |

What we reject: bordered cards around every event, uppercase "ARTIFACT" badges, raw JSON names
(`pull_request`), status lines repeating the agent's words, a filled blue user bubble, gradients
beyond the brand mark and the one soft glow behind a new chat's greeting (Gemini's home).

## Layout

- **Sidebar** 272 px, one step off the canvas (`--sidebar`), no border in light mode. Top: the mark
  and a collapse button; a "New chat" pill; the threads grouped by recency (Today, Yesterday,
  Previous 7 days, Previous 30 days, Older, by local calendar day) as single-line rows with a small live dot for a working or waiting
  thread. Collapsible on a desktop (remembered per browser); a sheet from the left on a phone.
- **Top bar** 56 px, transparent: the title (one line), the agent as a pill, the thread's state as a
  pill, and an overflow menu (Export JSON). On a phone the menu button opens the sheet.
- **Reading column** max 768 px (`max-w-3xl`), 16 px gutters on a phone, 24 px from `md`.
- **Composer** sticky at the bottom of the column, a 24 px-radius surface with a soft shadow: the
  text (1 to 8 lines), then a row with the agent pill on the left and a 36 px round Send / Stop
  button on the right. A one-line disclaimer under it.
- **Empty state** (new chat): the mark, a greeting and what the chosen agent does, the composer in
  the middle of the page over a faint radial glow of the accent, and suggestion chips under it (a
  chip fills the box, it does not send); the agent and release pickers are pills inside the
  composer.

## A turn

- **The person**: a soft bubble on the right (`--bubble`), 20 px radius with a 6 px corner at the top
  right, max 85 % of the column, markdown inside.
- **The agent**: its mark (a 28 px sparkle) and its name once, then, in order: the **steps**, its
  **words** as prose, and its **cards**. Nothing of the agent's sits in a bubble.
- **Steps** (always visible, never collapsed behind a toggle): a compact list, one 28 px line per
  step, an icon in a 20 px column joined by a hairline. The live step spins; finished steps are a
  quiet check; a failure is a cross. Labels are human: "Started working", "Preparing the workspace",
  "Pushed agent/fix", "Checks passed", "Opened pull request #12", "Checks failed — trying again
  (2/3)". A command (`$ …`) is monospace in a light box, one line, with Show more. Findings open in
  place. Past 30 steps the earliest fold behind "Show N earlier steps".
- **Before the first event** a shimmering "Coder is starting…" line under the mark.
- **Cards** (after the words): a pull request card (repository, number, title, branch chip, Open
  button), a file card; A2UI surfaces as they are. Errors are soft callouts in the flow, never
  alerts on replay.
- **Your turn**: the agent's question is its words, with a "Your turn" chip under them, and the
  composer says "Reply…".

## Type, spacing, radii

- Inter (variable, self-hosted via `@fontsource-variable/inter`), system monospace for code.
- Scale: 12 (meta), 13 (steps, chips), 14 (UI), 16/1.7 (prose and messages), 18 (top bar title),
  28/1.25 (greeting). Weights 400 and 500, 600 for titles only.
- Spacing on a 4 px grid: 4 between a step's parts, 8 inside chips, 16 inside cards, 28 between
  turns.
- Radii: 6 (inline code, small chips' inner), 12 (cards, code blocks), 20 (bubbles), 24 (composer),
  full (pills, icon buttons, avatar).

## Colour

Neutral canvas, one blue accent, semantic colours only for state (always with a word and an icon).

| Token | Light | Dark |
|---|---|---|
| `--background` | `#ffffff` | `#131314` |
| `--sidebar` | `#f3f5f8` | `#1b1c1e` |
| `--bubble` (user) | `#eef2f7` | `#2a2c2f` |
| `--card` | `#ffffff` | `#1c1d1f` |
| `--foreground` | `#1b1c1e` | `#e6e7e9` |
| `--muted-foreground` | `#5b5f66` | `#a3a7ad` |
| `--border` | `#e4e7ec` | `#2f3237` |
| `--primary` | `#0b57d0` | `#a8c7fa` |
| `--success` | `#137333` | `#81c995` |
| `--destructive` | `#b3261e` | `#f2b8b5` |
| `--warning` | `#8a5300` | `#f5c46a` |
| `--verifying` | `#6b3fa0` | `#cfb6f7` |

Every text colour is checked by axe (WCAG AA) in both schemes by `e2e/a11y.spec.ts`.

## Motion

A new turn fades in and rises 4 px (160 ms, ease-out); the live step spins; "starting" shimmers.
All of it is off under `prefers-reduced-motion`.

## Accessibility

The transcript is `role="log"` (name "Conversation"); the state pill is a polite `status`; steps are
a list whose items carry their full meaning as text; focus rings on every control; everything works
from the keyboard; axe finds nothing serious in either scheme.

## A surface that needs a newer version of the app

The thread was opened in a newer version of the app, and the agent used a component this one does not have (ADR 0023).
It is said, not half drawn: a quiet bordered card (`--card`, the same width as a surface) with a refresh icon, "This
part of the answer needs a newer version of the app." as its title, the component's name in muted text, and a small
outline **Reload** button; the agent's label stays in the corner. It is a labelled group, never an `alert`: a replay of
the thread must not announce it again. When the catalog is not newer, the same surface is the agent's mistake and is the
refusal line ("Interface not shown: ..."), in the destructive colour.
