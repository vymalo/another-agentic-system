# ADR 0004 — Protocols as closed enums, not a dynamic adapter registry

- **Status:** accepted (2026-09-28). Amended (2026-09-28) by ADR 0009: the set of `Event`/`Command` variants stays closed; the implementations behind each port are swappable at build time. Status note (2026-09-29): the decision stands. As built, the canonical inbound enum is named `Input` (in `orch-core`) and `Event` is the entry of the append-only log; `Command` is as described.

## Context

The orchestrator must accept input from and produce output to many protocols
(A2A, chat, MCP, webhooks, timers, Slack, GitHub, …), and the list will grow.

## Decision

Canonical `Event` and `Command` enums in a pure `core` crate; each protocol is
an adapter module translating at the edge. Outbound dispatch is a `match` over
`Command`, not a `HashMap<Channel, Box<dyn Adapter>>`.

## Why

- The set of channels is known at compile time; there is no runtime plugin
  requirement.
- Adding a variant makes the compiler list every place that must handle it —
  exactly the safety wanted as the protocol list grows.
- No vtables, no async-trait boxing (`async fn` in traits is not
  dyn-compatible).

## Consequences

Adding a protocol (a new `Event`/`Command` variant) is a code change plus a
release. Swapping the *implementation* of an existing port is not — see
ADR 0009.
