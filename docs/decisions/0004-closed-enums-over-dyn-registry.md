# ADR 0004 — Protocols as closed enums, not a dynamic adapter registry

- **Status:** accepted (2026-09-28)

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

Adding a protocol is a code change plus a release — acceptable for a
single-owner system. Revisit only if third parties need to add adapters.
