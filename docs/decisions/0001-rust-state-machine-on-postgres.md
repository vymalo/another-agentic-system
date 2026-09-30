# ADR 0001 — Orchestrator: a Rust state machine on Postgres

- **Status:** accepted (2026-09-28). Amended (2026-09-29) by
  [ADR 0015](0015-control-plane-and-workers-on-adam-rs.md): the job ledger also holds the journal of
  an in-process (local) agent, in `orch_agent_*` tables of the same Postgres. Processes stay
  stateless; the orchestrator now runs as a control plane and workers over this one database.
  Status note (2026-09-30): [ADR 0016](0016-inbox-timers-and-job-ledger-on-the-thread.md) makes the
  "transactional inbox" and timers of this decision concrete (planned, not built): the job ledger is
  a `threads.job` column committed with the state, the inbox carries webhooks and timers only, and MCP
  bypasses it ([ADR 0019](0019-mcp-server-over-streamable-http.md)). The decision stands.

## Context

Jobs are long-running (minutes to hours), wait on agents, CI and humans, and
must survive restarts. Candidates: eve (TypeScript durable agent framework on
the AI SDK), Restate (durable-execution engine written in Rust, Rust SDK), or a
state machine of our own.

## Decision

A Rust service implementing an explicit job state machine, persisted in the
Postgres (CNPG) we already run: transactional inbox/outbox, `SELECT … FOR
UPDATE SKIP LOCKED` for claiming, `LISTEN/NOTIFY` for wake-ups and live UI
updates.

## Why

- **Correctness over speed.** The orchestrator mostly waits on LLM calls and
  CI; its own overhead is irrelevant. What matters is that impossible
  transitions don't compile (exhaustive `match` over state and event enums)
  and that a crash anywhere resumes from the last committed state.
- **One datastore.** Postgres is already the chat store; making it the queue
  and the bus too avoids operating a broker or a second stateful engine.
- **Same stack** as our other Rust services (tooling, CI, conventions).

## Alternatives rejected

- **eve** — would add a second agent-hosting framework next to kagent; its
  self-hosted durability outside Vercel is unverified.
- **Restate** — capable, but the server is BSL-licensed and is another
  stateful system to run, back up and upgrade. Revisit if waits/timers outgrow
  a ~10-state machine.

## Consequences

We own retries, timers, idempotency and the outbox. They are small and
well-understood patterns; see [orchestrator.md](../orchestrator.md).
