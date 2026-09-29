# Orchestrator

A stateless Rust service that implements the chat API
([`docs/api/chat-api.yaml`](../docs/api/chat-api.yaml)) and delegates each
thread to one configured A2A agent. Design:
[`docs/orchestrator.md`](../docs/orchestrator.md); decisions: ADRs 0001, 0004,
0007, 0008, 0009 in [`docs/decisions/`](../docs/decisions/).

> **Status:** the pure core, the ports (with an in-memory implementation and a
> conformance testkit), the application service with its durable dispatcher and
> the HTTP API are implemented and tested. The A2A adapter, the Postgres store
> and the runnable binary land in the following slices of issue #9.

## Crates

| Directory | Package | Role |
|---|---|---|
| `crates/core` | `orch-core` | Pure: contract types (`ThreadState`, `Event`, `EventKind`, `Actor`, …) and `transition`. No async, no I/O. |
| `crates/ports` | `orch-ports` | Traits `ThreadStore`, `Wakeup`, `AgentClient`, `Clock`, `IdGen`; feature `testkit` adds in-memory implementations, a scripted fake agent and the conformance testkit. |
| `crates/app` | `orch-app` | Thread service (`transition` + optimistic commit loop, live event streams) and the durable outbox `Dispatcher`, written against the ports. |
| `crates/api` | `orch-api` | axum 0.8 routes for every operation of the contract, proxy-identity auth (fail closed), RFC 9457 problems, SSE. |

Dependency direction: `core` ← `ports` ← `app` ← `api`. Adapters (Postgres,
A2A) are separate crates that implement the ports; binaries only compose them
(ADR 0009).

## Behaviour worth knowing

- **Identity.** The API trusts `X-Auth-Request-Email` and answers 401 without it
  on every path except `/healthz` and `/readyz`. `AUTH_DEV_USER` (an e-mail)
  supplies an identity only when it is set. The header is only trustworthy
  behind a proxy such as oauth2-proxy that strips client-supplied copies.
- **`thread_state` events** are appended only when a thread *enters* `blocked`,
  `done`, `failed` or `cancelled`; entering `queued`/`working` is implied by
  `user_message` / `agent_status`.
- **No inbox table.** The chat API runs the transition inside the request and
  writes the events and the outbox row in one transaction, so redeliveries
  cannot happen on this path. The inbox of `docs/orchestrator.md` arrives with
  the webhook/MCP inputs.
- **Durability.** A delegation is an outbox row claimed under a lease. A worker
  that dies leaves the row to be re-claimed; `sent_at` tells the next worker to
  resume the agent's task (resubscribe, then poll) rather than send again, and
  every stored agent update carries an idempotency key.

## Test

```sh
cd orchestrator
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The contract conformance test (`crates/api/tests/conformance.rs`) starts the
real router on a TCP port over the in-memory stack, drives every operation and
validates each response body against the schemas of the contract.
