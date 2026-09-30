# orch-ports

One trait per infrastructure boundary of the orchestrator, plus a conformance
testkit and in-memory implementations.

## Where it sits

The **ports** of the ports-and-adapters split
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md),
[`docs/orchestrator.md`](../../../docs/orchestrator.md)). `orch-app` and
`orch-api` are written against these traits; the adapters are separate crates:
[`orch-store-postgres`](../store-postgres/README.md) (`ThreadStore`, `Wakeup`)
[`orch-agent-a2a`](../agent-a2a/README.md) and [`orch-agent-adam`](../agent-adam/README.md) (`AgentClient`). No
implementation type appears in a signature, and swapping happens at build time
in the composition root, not through runtime plugins. Depends on
[`orch-core`](../core/README.md) only.

## API at a glance

| Trait | What |
|---|---|
| `ThreadStore` | threads (with their job ledger: `Commit.job` is written in the same commit as the state, under the same version check, and `None` leaves it; a thread created without one has the default job, no gate), the per-thread event log with a strictly increasing `seq` (`commit` is atomic and version-checked), the A2A binding, and the outbox (`claim_outbox`, then `renew_lease`, `mark_sent`, `retry_outbox` and `complete_outbox`, each taking the claim's `Lease { id, owner, attempt }`, as does `Commit.lease`: the fencing token, so a worker whose row was claimed again is refused with `false` / `CommitOutcome::Fenced`; `OutboxItem::lease()` builds it from a claimed row; `skip_unsent_delegates`, `release_leases`, `get_outbox`, `list_open_outbox`, and `outbox_stats(now) -> OutboxStats { due, waiting, leased, oldest_due_at }`, the counts behind `/metrics`); `ping` for readiness |
| `Wakeup` | `notify(Topic)`, `subscribe()`, `capabilities()`; `Topic` is `Thread(ThreadId)`, `Outbox` or `Resync` (a hint only: the store is the truth) |
| `AgentClient` | `read_card` (`AgentCardInfo { description, version, releases, ui }`: `ui` is `UiSupport { versions }`, present only when the live card lists the A2UI extension), `send_stream` (a `SendRequest` carries `content: SendContent::{Text, UiAction { action, at }}`), `resubscribe`, `get_task`, `cancel`, `find_task_by_message`; an agent is an `AgentEndpoint { id, transport }` where `AgentTransport` is a closed enum (`A2a { card_url, bearer }`, whose `Debug` redacts the bearer, and `Local { name }`, an agent hosted in the orchestrator's own process; build one with `AgentEndpoint::a2a` or `AgentEndpoint::local`; both variants are always compiled) |
| `ByTransport<A, L>` | an `AgentClient` made of two: `a2a` serves `AgentTransport::A2a` endpoints, `local` serves `AgentTransport::Local`, decided by one exhaustive match in every method, so a new transport must be given a client before the workspace compiles. It holds no adapter type (`A` and `L` are any two `AgentClient`s); the binary builds it in a build with local agents |
| `Clock`, `IdGen` | time and identifiers; `SystemClock`, `UuidV7Ids` |
| `Ports`, `PortSet` | static-dispatch bundle of all five, chosen at build time |

Errors are `StoreError`, `WakeupError` and `AgentError`; each implements
`orch_core::Classify`, so callers decide retry and status by `ErrorClass`, not
by variant. The store and agent traits use `impl Future` methods (no
`async_trait`).

```rust
use orch_ports::{PortSet, Ports, SystemClock, UuidV7Ids};

// Composition happens at build time: pick one implementation per port.
let ports = PortSet { store, wakeup, agents, clock: SystemClock, ids: UuidV7Ids };
let _store = ports.store();
```

## Features

| Feature | Default | Effect |
|---|---|---|
| `testkit` | no | `memory::{MemoryStore, MemoryWakeup, ScriptedAgent, FixedClock, SeqIds, ..}`, and the conformance `testkit` with the macros `thread_store_conformance!`, `wakeup_conformance!` and `agent_client_conformance!`. `ScriptedAgent` runs the scripts `echo`, `ask`, `ui` (an A2UI surface in one payload, then `input-required`; the follow-up, an action or text, finishes the task; `set_ui(agent, versions)` changes what the card lists), `gate`, `slow`, `failed` (fails the task with a message; `fail` rejects the send) and `drop`, and `set_unreachable(agent)` makes every call to that agent fail as if nothing listened. Enable it as a **dev-dependency** feature in adapter crates |

## Tests

* `tests/memory_conformance.rs`: the testkit against the in-memory
  implementations (always runs). The in-memory store is the reference
  implementation of the suite.
* The store cases `stale_attempt_is_fenced`, `commit_after_another_owner_reclaims_is_fenced`,
  `commit_after_complete_is_fenced` and `expired_unclaimed_lease_still_commits` pin the fence.
* The store case `event_data_roundtrip` also pins the A2UI kinds and the `OutboxPayload::Action` row (a `delegate` row whose payload is an action).
* The store cases `job_roundtrip`, `job_is_written_with_the_state` and `gate_events_roundtrip` pin the job ledger: it comes back exactly from every read, a commit without a job leaves it, a refused commit (version conflict, replayed key, fenced lease) writes no job, one of several racing writers wins with its job, and the gate's events and the `verifying` state are stored.
* `tests/agent_conformance.rs`: the `AgentClient` testkit against `ScriptedAgent`.
* Unit tests in `src/` pin the error classification tables and `ByTransport` (`routes_by_transport_and_never_crosses`: two scripted agents, every operation once per transport, neither ever sees the other's endpoint).

Adapters run the same testkit; see
[`orch-store-postgres`](../store-postgres/README.md). To add a `ThreadStore`
or `Wakeup` implementation:

```rust
async fn make() -> Option<MyStore> { /* fresh, isolated store; None skips */ }
orch_ports::thread_store_conformance!(make);
```

An `AgentClient` implementation supplies an `AgentFixture` (the client, an endpoint of an agent that runs the scripts `echo`, `ask`, `gate`, `slow` and `fail`, an endpoint nobody listens on, and the gate the `gate` script waits for; override `text(script)` if the agent's words differ):

```rust
async fn make() -> Option<MyFixture> { /* client + a healthy agent; None skips */ }
orch_ports::agent_client_conformance!(make);
```

The calling crate needs `tokio` (with `macros` and `rt`) as a dev-dependency.

## See also

[`orch-core`](../core/README.md), [`orch-app`](../app/README.md),
[`orch-store-postgres`](../store-postgres/README.md),
[`orch-agent-a2a`](../agent-a2a/README.md).
