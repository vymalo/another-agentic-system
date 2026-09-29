# orch-app

The orchestrator application: the thread service (`App`) and the durable
outbox `Dispatcher`, written against the ports only.

## Where it sits

The application layer between [`orch-ports`](../ports/README.md) and
[`orch-api`](../api/README.md). `App<P: Ports>` runs
[`orch_core::transition`](../core/README.md) inside an optimistic commit loop
against a `ThreadStore`; `Dispatcher<P>` claims outbox rows and delegates them
to an agent through `AgentClient`. Because it depends on the traits only
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)),
any composition of adapters runs it; state lives in the store, not here
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
Design: [`docs/orchestrator.md`](../../../docs/orchestrator.md).

## API at a glance

| Item | What |
|---|---|
| `App::new(ports, AgentDirectory, AppConfig)` | the thread service; `App<P>` is shared as `Arc<App<P>>` |
| `App` operations | `list_agents`, `describe_agent(id)` (one configured agent and its live card, `AgentDescription { id, name, card: Option<AgentCardInfo> }`; `None` for an unknown id, a card that cannot be read in time gives `card: None`), `create_thread(user, NewThread)`, `list_threads`, `get_thread`, `list_events`, `post_message`, `cancel`, `find_thread(user, id)` (mine, free, or someone else's = `NotFound`), `create_thread_as(user, id, NewThread, Inbound)` (a thread id the caller chose; a taken id is `NotFound` for a stranger and `Creation::Exists` for the owner), `submit(user, thread, input, key)` (an already translated `Input` under an idempotency key, text validated), `apply(thread, input, key, binding, lease)` (feed an `Input`, retrying on version conflicts; the API passes `lease: None`, the dispatcher its claim, and a claim that is no longer current gives `ApplyOutcome::Fenced`, not retried), `record_binding` (same `lease`), `outbox_stats()` (the open outbox rows counted at the clock's now, returned with that now; behind `/metrics`), `event_stream(user, thread, after)` (replay then live, no gaps or duplicates; wakeups make it prompt, a poll makes it correct) |
| `App` lifecycle | `set_ready`, `is_ready`, `set_shutting_down`, `is_shutting_down`, `ports()`, `directory()` |
| `AppConfig` | `card_timeout` (3 s), `stream_poll` (5 s), `max_commit_attempts` (8) |
| `NewThread`, `Inbound` (`message_id`, `run_id`, `key` a surface records in the log), `Creation` (`Created` / `Exists`), `ApplyOutcome` (`Applied` / `Duplicate` / `Fenced`) | request and result types |
| `AgentDirectory`, `AgentEntry` | the static set of agents users can target (from configuration) |
| `Dispatcher::new(app, DispatcherConfig, owner)`, `run(shutdown)` | the durable outbox worker: concurrency, lease and heartbeat, retry with backoff, adaptive polling, cancel retries; every write it makes is fenced with the claim's attempt, so a worker paused past its lease drops its late result instead of writing it |
| `AppError` | classified via `orch_core::Classify` |

```rust
use std::sync::Arc;
use orch_app::{AgentDirectory, App, AppConfig, Dispatcher, DispatcherConfig};
use tokio_util::sync::CancellationToken;

let app = Arc::new(App::new(ports, AgentDirectory::new(entries), AppConfig::default()));
let dispatcher = Dispatcher::new(app.clone(), DispatcherConfig::default(), "replica-1");
let shutdown = CancellationToken::new();
tokio::spawn(dispatcher.run(shutdown.clone()));
```

## Features and environment

No Cargo features. The crate reads no environment variables; the binary maps
`DISPATCHER_CONCURRENCY`, `OUTBOX_LEASE_SECS` and friends onto
`DispatcherConfig` (see [`orchestrator`](../../bin/orchestrator/README.md)).

## Tests

Offline: they use the in-memory implementations of `orch-ports` (feature
`testkit`). No environment variables.

* `tests/service.rs`: the thread service without a dispatcher (validation,
  isolation between users, streams, idempotency, consumer-chosen thread ids and their collisions, `submit`; `list_agents` reads live cards,
  fails closed and keeps configuration order, so the first agent stays the default; `describe_agent` is one agent with its card, live, `None` card when unreadable).
* `tests/dispatcher.rs`: the dispatcher against the scripted agent, including a late result after another claimer (or the same owner name) re-claimed the row, with a heartbeat that never fires so only the fence can stop the old worker. Each claimed row is processed inside an `outbox` span (`id`, `thread`, `kind`, `attempt`), so every log line of that work carries them.
* `tests/restart.rs`: two app instances over one shared in-memory "database",
  the first killed mid-stream.

Against real HTTP and Postgres, see [`orch-e2e`](../e2e/README.md).

## See also

[`orch-ports`](../ports/README.md), [`orch-api`](../api/README.md),
[`orch-e2e`](../e2e/README.md).
