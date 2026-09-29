# ADR 0006 — Chat surface: Next.js + assistant-ui with an external store

- **Status:** accepted (2026-09-28). Amended (2026-09-29): the UI is built with shadcn/ui and
  the pruned assistant-ui registry components, in a feature layout; see
  [ADR 0011](0011-web-shadcn-tailwind-feature-layout.md).

## Context

The chat is not "you ↔ one model". It is a live view of a job: planner,
workers, CI and reviewers all post into it, and your replies unblock it.

## Decision

Next.js with [assistant-ui](https://www.assistant-ui.com/) (MIT) using
`useExternalStoreRuntime`: the app owns the messages, assistant-ui renders
them.

- **Messages** come from the job's `events` table, mapped through
  `convertMessage`.
- **Threads** come from our own database via a custom
  `ExternalStoreThreadListAdapter` — no Assistant Cloud.
- **Agent events render as cards:** custom `data-*` message parts
  (e.g. `{ type: "data-ci-result", data: {…} }`) plus `metadata` for the
  posting agent.
- **`onNew`** turns your message into an inbox event (e.g. answering a
  `Blocked` job); **`isRunning`** reflects whether the job is active.
- **Live updates:** Postgres `NOTIFY` → Next.js `LISTEN` → SSE → browser.

## Consequences

The chat is the job's history — the event log is the single source for both
the UI and replay.
