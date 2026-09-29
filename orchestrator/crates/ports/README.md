# orch-ports

One trait per infrastructure boundary of the orchestrator, plus a conformance
testkit and in-memory implementations.

## Where it sits

The **ports** of the ports-and-adapters split
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md),
[`docs/orchestrator.md`](../../../docs/orchestrator.md)). `orch-app` and
`orch-api` are written against these traits; the adapters are separate crates:
[`orch-store-postgres`](../store-postgres/README.md) (`ThreadStore`, `Wakeup`)
and [`orch-agent-a2a`](../agent-a2a/README.md) (`AgentClient`). No
implementation type appears in a signature, and swapping happens at build time
in the composition root, not through runtime plugins. Depends on
[`orch-core`](../core/README.md) only.

## API at a glance

| Trait | What |
|---|---|
| `ThreadStore` | threads, the per-thread event log with a strictly increasing `seq` (`commit` is atomic and version-checked), the A2A binding, and the outbox (`claim_outbox`, then `renew_lease`, `mark_sent`, `retry_outbox` and `complete_outbox`, each taking the claim's `Lease { id, owner, attempt }`, as does `Commit.lease`: the fencing token, so a worker whose row was claimed again is refused with `false` / `CommitOutcome::Fenced`; `OutboxItem::lease()` builds it from a claimed row; `skip_unsent_delegates`, `release_leases`, `get_outbox`, `list_open_outbox`, and `outbox_stats(now) -> OutboxStats { due, waiting, leased, oldest_due_at }`, the counts behind `/metrics`); `ping` for readiness |
| `Wakeup` | `notify(Topic)`, `subscribe()`, `capabilities()`; `Topic` is `Thread(ThreadId)`, `Outbox` or `Resync` (a hint only: the store is the truth) |
| `AgentClient` | `read_card`, `send_stream`, `resubscribe`, `get_task`, `cancel`, `find_task_by_message`; an agent is an `AgentEndpoint { id, transport }` where `AgentTransport` is a closed enum (today `A2a { card_url, bearer }`, `Debug` redacts the bearer; build one with `AgentEndpoint::a2a`) |
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
| `testkit` | no | `memory::{MemoryStore, MemoryWakeup, ScriptedAgent, FixedClock, SeqIds, ..}`, and the conformance `testkit` with the macros `thread_store_conformance!` and `wakeup_conformance!`. Enable it as a **dev-dependency** feature in adapter crates |

## Tests

* `tests/memory_conformance.rs`: the testkit against the in-memory
  implementations (always runs). The in-memory store is the reference
  implementation of the suite.
* The store cases `stale_attempt_is_fenced`, `commit_after_another_owner_reclaims_is_fenced`,
  `commit_after_complete_is_fenced` and `expired_unclaimed_lease_still_commits` pin the fence.
* Unit tests in `src/` pin the error classification tables.

Adapters run the same testkit; see
[`orch-store-postgres`](../store-postgres/README.md). To add a `ThreadStore`
or `Wakeup` implementation:

```rust
async fn make() -> Option<MyStore> { /* fresh, isolated store; None skips */ }
orch_ports::thread_store_conformance!(make);
```

The calling crate needs `tokio` (with `macros` and `rt`) as a dev-dependency.

## See also

[`orch-core`](../core/README.md), [`orch-app`](../app/README.md),
[`orch-store-postgres`](../store-postgres/README.md),
[`orch-agent-a2a`](../agent-a2a/README.md).
