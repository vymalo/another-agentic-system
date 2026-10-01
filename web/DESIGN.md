# Chat surface: design brief

The owner, after trying the previous version: "It should feel like a classical chat, not a machine
to machine chat interface", "It should look like a gemini chat, with custom components … very cute
and simple", with the agent's steps always visible. A day later, on seeing it (2026-10-01): "The UI
is currently TOO google gemini-like… change the icon to something custom for us… The idea of the
logo is 'boring giant panda, tri-color'." So the structure stays (a classical chat) and the look is
ours: the panda, ink actions, one bamboo green, warm neutrals (see "Brand" and "Colour"). This brief
is the contract the components in `src/` follow; the screenshots in `e2e/__screens__/`
(`pnpm screens`) show the result.

## References

Studied on [Refero](https://refero.design) on 2026-09-30, full screen images viewed. We take the
structure, not the branding.

| Reference | What we took |
|---|---|
| ChatGPT, conversation ([2ed2ffc3](https://refero.design/pages/2ed2ffc3-95b2-417c-a3fa-df50f4cb22e4)) | Centered reading column around 760 px; title in a minimal top bar with the actions on the right; a disclaimer line under the composer; ink (near-black) for the primary action; **the model picker**: a button in the top bar that names the choice ("GPT ⌄") and opens a menu of one row per model, a name with a line under it and a check on the current one (our agent picker, see "Agent picker") |
| Claude ([f42e56ca](https://refero.design/pages/f42e56ca-6f9d-45b8-b501-ce30add0f259)) | "Reply to Claude…" placeholder for the next turn; the model named inside the composer; warm neutrals |
| Meta AI ([6c1e2ac6](https://refero.design/pages/6c1e2ac6-e65a-4290-8ae5-81ad61ea200a)) | The assistant's avatar once above its turn, then prose; generous spacing between turns |
| Copilot ([d81060e5](https://refero.design/pages/d81060e5-9955-4fb2-a8d6-b4ccaa97c312)), Grok ([0ac86436](https://refero.design/pages/0ac86436-063a-45a1-a04d-81042b245675)) | A composer with a soft shadow and chips inside it; a two-line greeting (statement, then a muted question) |
| Cursor agents (flow [9922](https://refero.design/flows/9922): [16261b66](https://refero.design/pages/16261b66-bd9b-4403-94da-7d88c4c15157), [8b09e132](https://refero.design/pages/8b09e132-8a3e-4c4b-b8cf-617b51ba3129), [d7a49941](https://refero.design/pages/d7a49941-aef5-4f63-8d48-a6e66930e104)) | An agent's steps as quiet single lines ("Completing setup", commands in monospace in a light box) between its messages; the follow-up composer names the model |

What we reject: bordered cards around every event, uppercase "ARTIFACT" badges, raw JSON names
(`pull_request`), status lines repeating the agent's words, a filled user bubble, decorative
gradients and glows (the fade under the scroll and the shimmer of "starting" are function, not
decoration).

### What we left behind

The first version took its look from Gemini and Bard; the owner found it too much like them
(2026-10-01, quoted above). The structure those screens taught is still ours; their look is not.

| Reference | What we took then | What we do now |
|---|---|---|
| Gemini, home ([965e3a7f](https://refero.design/pages/965e3a7f-d7dc-45ae-8cc1-12025b10b1fd), [6994cae8](https://refero.design/pages/6994cae8-03f9-4ef0-9fb5-95a45469cf52)) | Quiet left sidebar (New chat, Recent), a centered greeting over one pill-shaped composer, lots of air; a faint radial glow behind the greeting | The sidebar, the greeting and the composer stay. The glow is gone (a plain canvas) and the greeting is "What should we get done?" under the panda |
| Gemini, conversation ([dc0889ae](https://refero.design/pages/dc0889ae-1e9f-49de-8465-5d810e0d6f18)) | The user's words in a soft grey bubble on the right; the answer as plain prose under a small label, no bubble; a floating rounded composer with a round send button | Same, in warm neutrals; the round send button is ink, not blue |
| Bard, dark ([195db751](https://refero.design/pages/195db751-c533-4953-9247-824167e79f03)) | Dark mode: near-black canvas, the sidebar one step lighter, the four-pointed sparkle as the assistant's avatar | A warm near-black canvas (`#141614`); the avatar is the agent's own letter, the panda is the product's |
| Material blue accent, the sparkle's blue-to-pink gradient, a pastel tile behind it | `#0b57d0` / `#a8c7fa`, `#131314` / `#1b1c1e` | Ink for actions and one bamboo green (`--brand`) for what is ours |

## Brand

The mark is a **boring giant panda, in three colours**: bamboo green `#3F7341`, ink `#161D17`, cream
`#FAF7EF`; half-closed eyes, a bamboo leaf in its mouth. It is the owner's drawing, traced to one SVG
(`public/brand/panda.svg`, a 512 viewBox, about 12 KB, exactly three fills, clipped to a disc, no outer
ring). The fills do not change with the colour scheme. Decision (owner, 2026-10-01): **the full mark
at every size**, with its body and bamboo; a head-only drawing was made and is not shipped.

| Where | What |
|---|---|
| Sidebar lockup | the mark at 28 px and the wordmark `another·agentic`: lower case, Inter 600, 15 px, tight tracking, the dot in `--brand` (`components/brand/panda-mark.tsx`) |
| New chat | the mark at 72 px (96 px from `sm`) over the greeting |
| Tab icon | `src/app/icon.svg` and `favicon.ico` (16, 32, 48 px): the same drawing framed 1.25 times closer and cropped to a circle round the face, because at 16 px the whole body is a smudge and the face is still a panda |
| iOS | `src/app/apple-icon.png`, 180 px: the mark on a full square of the green (iOS fills transparency with black) |
| Install | `src/app/manifest.ts`: `another·agentic`, theme `#3F7341`, icons `public/brand/icon-192.png`, `icon-512.png` and the maskable `icon-maskable-512.png` (the mark inside the 80 % safe circle, on the green) |
| Browser chrome | `viewport.themeColor`: white in light, `#141614` in dark |
| README | the repository README shows `public/brand/panda.svg` at 96 px |

`pnpm brand:icons` (`scripts/brand-icons.mjs`) regenerates `icon.svg`, `favicon.ico`, `apple-icon.png` and
the three manifest icons from `public/brand/panda.svg`, rendered by the Chromium Playwright already
pins; `components/brand/brand-assets.test.ts` holds the files to three fills, no `<image>`, no
`<script>`, 16 KB and the right pixel sizes.

**The panda is the house, not an agent.** An agent is a 28 px circle with the first letter of its
name (Inter 600, 13 px) on one of six sober tints (sage, sand, slate, clay, plum, teal), chosen by a
stable hash of the agent's id, so an agent keeps its colour everywhere (`components/brand/agent-avatar.tsx`;
every letter is at least 4.5:1 on its tint, light and dark). It is `aria-hidden`: the name is text
beside it. The orchestrator's own lines (checks, CI) keep their step icons. The panda itself has no name.

## Layout

- **Sidebar** 272 px, one step off the canvas (`--sidebar`), no border in light mode. Top: the panda
  with the wordmark, and a collapse button; a "New chat" pill; the threads grouped by recency (Today, Yesterday,
  Previous 7 days, Previous 30 days, Older, by local calendar day) as single-line rows with a small live dot (green when working, amber when waiting) for a working or waiting
  thread. Collapsible on a desktop (remembered per browser); a sheet from the left on a phone.
- **Top bar** 56 px, transparent: the **agent picker** (a button with the agent's name and a chevron,
  see "Agent picker"), the title (one line, muted, from `md`; below it the title is for screen readers
  only, and stays the page's heading), the thread's state as a pill, the **panel toggle** (a thread only,
  see "Panel") and an overflow menu (Export JSON). On a phone the menu button opens the sheet, in front
  of the picker.
- **Panel** on the right of a thread: 360 px by default, docked beside the chat on a wide window and a
  sheet on a narrower one; "Panel" says how it behaves. It is what makes the reading column calm: the
  agents' work and what they shared are beside the conversation, not in it.
- **Reading column** max 768 px (`max-w-3xl`), 16 px gutters on a phone, 24 px from `md`.
- **Composer** sticky at the bottom of the column, a 24 px-radius surface with a soft shadow: the
  text (1 to 8 lines), then a row with a 36 px round Send / Stop button on the right; the left of the
  row is empty and kept for the tools picker and mentions (plan 05): the agent is picked in the top
  bar, not in the box. A one-line disclaimer under it.
- **Empty state** (new chat): the panda, the greeting "What should we get done?" and what the chosen
  agent does, the composer in the middle of the page on the plain canvas (no glow), and suggestion
  chips under it (a chip fills the box, it does not send); the agent picker is in the top bar, in the
  same place as on a thread.

## Agent picker

Choosing the agent is like choosing a model in ChatGPT, not a form field (owner, 2026-10-01: "a little more
like the new ChatGPT"). It is a menu button in the top bar, on the new chat and on every thread
(`features/agents/components/agent-menu.tsx`).

- **The button**: ghost, 36 px, the agent's name (16 px, weight 500), then muted "· production" when the
  agent offers releases (the one that will be used, the default channel until another is chosen; a phone's
  top bar has no room for it, so there it is read but not drawn), then a chevron; it gives way (its text is
  cut) before the state and the toggle do. Its name is "Agent: Coder · production" (a menu button: `aria-haspopup="menu"`,
  `aria-expanded`). While the list loads it is a skeleton; if it could not be read it says "Agents
  unavailable" and the menu holds Retry.
- **The menu** (300 to 380 px, `--popover`, 12 px radius, shadow like the composer's): the label "Agents",
  then one **radio item** per agent (`role="menuitemradio"`, `aria-checked`): its avatar (24 px), its
  name (14 px, weight 500) and what it does in one muted line (12 px, from its live card), and a check
  in `--brand` on the chosen one. When the chosen agent offers releases, a separator and a second group,
  "Release": the channels (`production — coder-r47`) and then the revisions, in the **same menu**, not
  a sub-menu, so it works from a phone and from the keyboard. Choosing an item closes the menu. The list
  is read again each time the menu opens: releases are the agent's card right now (ADR 0008).
- **Keyboard**: Enter, Space or the down arrow on the button opens it; the arrows move over the items,
  letters jump to an agent, Enter or Space chooses, Escape closes it and the focus goes back to the button.
- **On an existing thread** a thread has one agent, so the menu shows it (checked, with its pinned
  release) and offers the others as **"Start a new chat with …"** links: they open a new chat with that
  agent already chosen (`/?agent=reviewer`). They are not radio items, because nothing in this chat changes.
  Changing the agent of a thread will be a fork (plan 08, F-series): the conversation so far copied into a
  new thread that talks to the other agent; until it is built, this is what the menu offers.
- **A notice slot** under the lists, inside the menu: a failed refresh of the list is said there ("Could not
  refresh the agents: …", with Retry, the list stays as it was), and the agent registry's "unreachable,
  showing the configured agents only" (plan 05) goes in the same place.

## Panel

The right side of a thread was unused (owner, 2026-10-01: "the right side of the page is usually unused … add a
collapsible right panel to show the sources; the agents (and sub-agents) work better with a cleaner interface").
The panel is the thread's second surface, `features/panel/`: two tabs, **Activity** and **Sources**.

- **Where.** From 1132 px (the sidebar, 560 px of reading column and the narrowest panel) the panel is
  **docked**: an `<aside>` between the chat and the edge, `--background` with a hairline on its left. Its
  width is 360 px by default, 300 to 560 px, never more than 45 % of the window and never so much that the
  column has less than 560 px beside the sidebar (448 px at 1280). Below 1132 px it is a **sheet**: from the
  right from 768 px, from the bottom (85 % of the height, rounded top) on a phone, the way the thread list
  is a sheet. Neither is on the new-chat page.
- **Open or closed.** The header's toggle (an icon, "Thread details", `aria-expanded`, `aria-controls`) and
  **Ctrl/⌘+Shift+.** (the physical key, from anywhere, the message box included) toggle it, and so does the
  panel's own close button. A docked panel is remembered per browser (`chat.panel`, `chat.panel.width`,
  `chat.panel.tab` in localStorage; a head script sets `data-panel` on `<html>` before the first paint, as the
  sidebar's does); with nothing remembered it is open from 1280 px and closed below. A sheet is never open by
  itself and never remembered: it is for this visit.
- **Activity** is where the step tree goes (plan 03, S5.4): one section per agent turn, fed by the shell's
  small contract, `useStepsPanel()` (`hooks/use-steps-panel.tsx`: the panel's id, whether it shows, the tab, a
  request to focus a turn, and `openSteps(turnId)`). Until the tree exists the steps stay in the
  conversation, as "A turn" says, and the tab says so.
- **Sources** is what the agents shared, derived in the browser from what the runtime already holds: the pull
  requests and branches they opened or pushed, their CI reports that link to a run, the files with a link,
  and the links in their words. Four groups (Pull requests & branches, Checks, Files, Links), each item once
  however often it was cited, with a **Turn n** button for each turn that cited it: it scrolls the chat to
  that turn and focuses its header (on a phone the sheet closes first). A typed source (a pull request, a
  report) describes a URL better than the same URL in someone's words, so it takes the place of the link.
  Rules, because it is all agent output: only absolute http(s) URLs are ever links (`safeHttpUrl`), a URL
  inside code is not read, nothing is fetched (no favicons), every link opens in a new tab with "(opens in a
  new tab)" for a screen reader, and titles are text. A branch has no link: the projection's repository is
  `host/owner/name`, not a URL.
- **Look.** A header 48 px high with the tabs (14 px, weight 500, the selected one in the page's ink with a
  2 px `--brand` underline) and a close button; the Sources count in muted text after the name. An item is a
  32 px round icon on `--muted` (a CI report's is green or red, and says it in words too), its title (14 px,
  weight 500, underlined when it is a link, an arrow after it), a muted line (the kind and a host, a
  provider or a type) and the Turn buttons (28 px, outlined). An empty tab is the panda at 96 px, what the
  tab is for, and why it is empty.
- **Resizing.** The edge between the chat and the panel is a window splitter: a vertical `separator` the
  keyboard can focus (Left widens by 16 px, Right narrows, Home and End go to the limits; its value is the
  width in px) and a pointer can drag (the width does not animate while it is dragged).
- **Motion.** The docked width animates in 200 ms; off under `prefers-reduced-motion`.

## A turn

- **The person**: a soft bubble on the right (`--bubble`), 20 px radius with a 6 px corner at the top
  right, max 85 % of the column, markdown inside.
- **The agent**: its avatar (a 28 px circle with its first letter, see "Brand") and its name once, then, in order: the **steps**, its
  **words** as prose, and its **cards**. Nothing of the agent's sits in a bubble.
- **Steps** (always visible, never collapsed behind a toggle): a compact list, one 28 px line per
  step, an icon in a 20 px column joined by a hairline. The live step spins; finished steps are a
  quiet check; a failure is a cross. Labels are human: "Started working", "Preparing the workspace",
  "Pushed agent/fix", "Checks passed", "Opened pull request #12", "Checks failed — trying again
  (2/3)". A command (`$ …`) is monospace in a light box, one line, with Show more. Findings open in
  place. Past 30 steps the earliest fold behind "Show N earlier steps".
- **Before the first event** a shimmering "Coder is starting…" line under the avatar.
- **Cards** (after the words): a pull request card (repository, number, title, branch chip, Open
  button), a file card; A2UI surfaces as they are. Errors are soft callouts in the flow, never
  alerts on replay.
- **Your turn**: the agent's question is its words, with a "Your turn" chip under them, and the
  composer says "Reply…".
- **Choices** (a surface component, below): the agent's several questions with fixed answers, in the
  surface's card; **the person's answers** to them are a bubble, see below.

## Type, spacing, radii

- Inter (variable, self-hosted via `@fontsource-variable/inter`), system monospace for code.
- Scale: 12 (meta), 13 (steps, chips), 14 (UI), 16/1.7 (prose and messages), 18 (top bar title),
  28/1.25 (greeting). Weights 400 and 500, 600 for titles only.
- Spacing on a 4 px grid: 4 between a step's parts, 8 inside chips, 16 inside cards, 28 between
  turns.
- Radii: 6 (inline code, small chips' inner), 12 (cards, code blocks), 20 (bubbles), 24 (composer),
  full (pills, icon buttons, avatar).

## Colour

A warm neutral canvas, **ink for the actions** (the send button, primary buttons: near-black in light,
near-white in dark), **one bamboo green, `--brand`, for what is ours** (links, the focus ring, the live
dot, the dot in the wordmark, the live step, a chosen option), and semantic colours only for state (always with a word
and an icon). The working badge is neutral ink: the green says "alive", the words say what.

| Token | Light | Dark |
|---|---|---|
| `--brand` | `#3f7341` | `#8dc58b` |
| `--background` | `#ffffff` | `#141614` |
| `--sidebar` / `--sidebar-accent` | `#f6f5f0` / `#eae8e0` | `#1b1e1b` / `#2a2e2a` |
| `--bubble` (user) | `#f1f0ea` | `#282c28` |
| `--card` / `--popover` | `#ffffff` / `#ffffff` | `#1a1d1a` / `#222622` |
| `--foreground` | `#1a1d1a` | `#e8e9e4` |
| `--muted-foreground` | `#5c6058` | `#a6aba3` |
| `--muted` / `--secondary` | `#f3f2ec` | `#232723` |
| `--border` / `--input` | `#e7e5dd` / `#dad8cf` | `#2e332e` / `#3a3f3a` |
| `--primary` (ink) / `--primary-foreground` | `#1a1d1a` / `#ffffff` | `#f2f1ea` / `#141614` |
| `--ring` | `var(--brand)` | `var(--brand)` |
| `--success` | `#137333` | `#81c995` |
| `--destructive` | `#b3261e` | `#f2b8b5` |
| `--warning` | `#8a5300` | `#f5c46a` |
| `--verifying` | `#6b3fa0` | `#cfb6f7` |

`--brand` as text is at least 4.5:1 on every surface it sits on (5.6:1 on white; 6.9:1 or better in
dark). Every text colour is checked by axe (WCAG AA) in both schemes by `e2e/a11y.spec.ts`.

## Motion

A new turn fades in and rises 4 px (160 ms, ease-out); the live step spins; "starting" shimmers.
All of it is off under `prefers-reduced-motion`.

## Accessibility

The transcript is `role="log"` (name "Conversation"); the state pill is a polite `status`; steps are
a list whose items carry their full meaning as text; focus rings on every control; everything works
from the keyboard; axe finds nothing serious in either scheme.

The panel is a `complementary` landmark named "Thread details" (hidden and inert while it is closed, so it is
neither tabbed into nor read); in a sheet it is a dialog with the same name that traps the focus, closes
with Escape and gives the focus back to what opened it (to the turn, when a Turn button closed it). Its
tabs are a `tablist` with the arrows, Home and End; its edge is a `separator` with `aria-valuenow`, `min` and
`max`; its toggle keeps one name and says its state in `aria-expanded`, and announces its shortcut
(`aria-keyshortcuts`). The agent picker's menu is not modal (axe reports the siblings of a modal menu as
focusable though hidden), and axe is run with the picker's menu open and with the panel open, both tabs,
in both schemes.

## A surface that needs a newer version of the app

The thread was opened in a newer version of the app, and the agent used a component this one does not have (ADR 0023).
It is said, not half drawn: a quiet bordered card (`--card`, the same width as a surface) with a refresh icon, "This
part of the answer needs a newer version of the app." as its title, the component's name in muted text, and a small
outline **Reload** button; the agent's label stays in the corner. It is a labelled group, never an `alert`: a replay of
the thread must not announce it again. When the catalog is not newer, the same surface is the agent's mistake and is the
refusal line ("Interface not shown: ..."), in the destructive colour.

## Choices and the person's answers

`Choices` is the component of the UI catalog for "ask several things at once" (ADR 0023). It sits in the surface's card, as
quiet as the rest of the page, and is a form in the plainest sense:

- Each question is a **group** named by its words (the legend: 14 px, weight 500; "(optional)" in muted text after it when it need
  not be answered). Its options are **tiles**: a 12 px-radius, 1 px `--border` row at least 44 px tall (a thumb, not a pointer),
  the native radio button (a check box for several) in `--brand` at 16 px, the label, and the option's description under it in
  12 px muted text. The **whole tile** is the click target; a chosen one takes a `--brand` border and a 5 % `--brand` wash; the
  focus ring is the page's ring (`ring-3`, `--ring` at 50 %), on the tile.
- **Other** is a tile of its own: the choice, "Other", and a text box beside it; typing in the box turns the choice on.
- Under the questions, **one** primary button ("Send answers", or the agent's words) that waits for every required question, and a
  12 px muted line that says how many are left. Enter in a text box does nothing: only the click on the button sends.
- When the thread does not wait for the person (it is working, it is finished, or this is not the newest copy of the surface), the
  tiles stay as they were and stop taking input, and the surface says once why, as for any button.
- It is keyboard complete with native controls: Tab to a group, the arrows to choose, Space to tick, Tab to the button.

**The answers** are the person's, so they are drawn as the person's words are: a `--bubble` on the right (20 px radius, the 6 px top
right corner, at most 85 % of the column), **above the agent's mark** of the turn they started (the agent has not yet said anything
when the person answers). It is headed "Your answers" (12 px, weight 500, muted) and lists each question in 13 px muted text with
the labels chosen under it in the body size, "Other: ..." for their own words and "No answer" for a skipped optional question. It
is not a step of the agent's list: nothing the person did is the agent's activity. When the surface the answer names is no longer
in the transcript, the question's id and the raw value stand in.

## Cards and Mermaid

Two components of the UI catalog (version 3, ADR 0023) for what an agent shows. They sit in the surface's card like `Choices`, as
quiet as the rest of the page: no colour of their own, nothing that moves, nothing that loads.

- **Cards** are tiles inside the surface: 12 px radius, 1 px `--border`, `--background` fill (one step off the surface's `--card`), 14 px
  of padding. In a **list** they stack with an 8 px gap; a **grid** is two columns from `sm`. Inside a tile: the **title** (14 px, weight
  500; when the card has a link the title is the link, in `--brand` and underlined, with "(opens in a new tab)" for a screen reader), the
  **subtitle** under it (12 px, `--muted-foreground`), the **body** (14 px, line breaks kept), and a last row of **tags** as small
  `--secondary` chips with the **host** of the link at the end (12 px muted, an external-link icon): the person sees where a link goes
  before they follow it. A card never has an image, a favicon or a preview (ADR 0013 rule 5): where an image would be, there is nothing.
  A list or grid may have a title in the surface's heading style (16 px, weight 600).
- **Mermaid** is a figure: an optional **title** (the same heading style), the **picture**, an optional **caption** (12 px muted) and a
  quiet disclosure, "Diagram source" (12 px muted, a triangle), that opens the graph's text in the page's code block (12 px radius
  corners at 6, `--muted` fill, monospace, scrolls and takes focus). The picture is drawn **in the page's own tokens**, so a graph is a
  neighbour of the cards and not a screenshot of another app: nodes `--muted` with a `--input` border, text `--foreground`, lines
  `--muted-foreground`, the surface's `--card` behind; dark mode is the dark tokens (`prefers-color-scheme`, as everywhere). **Plain
  shapes**: mermaid's `classic` look, no drop shadows, no gradients, no hand-drawn style. At most its natural size and never wider than
  the card: on a phone it scales down with its text, and the source is the way to read it at full size.
- **A graph that cannot be drawn** is the same figure with a destructive-toned callout in place of the picture: a circle-alert icon,
  "The graph could not be drawn.", the first three lines of mermaid's message in the destructive colour, and the source in the code block
  (always open: it is what the person and the agent need to see). It is not an `alert` (a replay must not announce it again), and it does
  not take the surface down: the words and cards beside it stay.
- **Loading**: until mermaid has drawn, "Drawing the graph…" (12 px muted) holds the picture's place at least 96 px high, and the
  source disclosure is already there.
- Accessibility: the picture is an `<img>` named by the title (else the caption, else "Diagram"); the source disclosure is its text
  alternative; cards are a list of list items; every link says it opens a new tab; the focus ring is the page's.

