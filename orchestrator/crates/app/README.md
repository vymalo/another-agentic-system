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
| `App` operations | `list_agents`, `describe_agent(id)` (one configured agent and its live card, `AgentDescription { id, name, card: Option<AgentCardInfo> }`; `None` for an unknown id, a card that cannot be read in time gives `card: None`), `create_thread(user, NewThread)`, `list_threads`, `get_thread`, `list_events`, `post_message`, `cancel`, `find_thread(user, id)` (mine, free, or someone else's = `NotFound`), `create_thread_as(user, id, NewThread, Inbound)` (a thread id the caller chose; a taken id is `NotFound` for a stranger and `Creation::Exists` for the owner), `submit(user, thread, input, key)` (an already translated `Input` under an idempotency key, text or action validated: an oversized or malformed `Input::UiAction` is `AppError::Invalid` and nothing is written), `apply(thread, input, key, binding, lease)` (feed an `Input`, retrying on version conflicts; the API passes `lease: None`, the dispatcher its claim, and a claim that is no longer current gives `ApplyOutcome::Fenced`, not retried), `record_binding` (same `lease`), `outbox_stats()` (the open outbox rows counted at the clock's now, returned with that now; behind `/metrics`), `event_stream(user, thread, after)` (replay then live, no gaps or duplicates; wakeups make it prompt, a poll makes it correct) |
| `App` lifecycle | `set_ready`, `is_ready`, `set_shutting_down`, `is_shutting_down`, `ports()`, `directory()` |
| `AppConfig` | `card_timeout` (3 s), `stream_poll` (5 s), `max_commit_attempts` (8), `gate` (the deployment's `GatePolicy`, the base a new thread's job starts from; the default requires nothing, which is the behaviour from before the gate existed), `target_gates` (an agent's `gate` key of `AGENTS_FILE`, by agent id) and `gate_rules` (`GateRules`: which sources this build honours and the attempts cap) |
| `GateLayer`, `GateRules`, `Layer`, `GateError`, `SourceName`, `CiLayer`, `pending_reason`, `THREAD_GATE_KEY`, `DEFAULT_MAX_ATTEMPTS_CAP` | the gate's configuration ([ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md)), in `gate_config.rs`. A `GateLayer` is `{require?, maxAttempts?, verifier?, ci?}` (`deny_unknown_fields`, camelCase; sources are spelled `ci`, `agent-checks`, `verifier`), read from YAML (`AGENTS_FILE`), JSON (`GateLayer::from_json`, the `forwardedProps["vymalo.gate"]` value) or built from the environment. `GateRules::apply(above, layer, at)` puts a layer on the policy above it and refuses: a source or setting this build cannot honour (`ci`, `verifier`: `pending_reason` says which slice enables it, and is the one place a slice changes), a `require` that leaves out a source the layer above requires, `maxAttempts` outside `1..=cap`, a `verifier` or `ci` per thread. `GateRules::validate` checks every agent's resolved gate at startup, and that each verifier is another configured agent. `App::resolve_gate(agent, request)` is the deployment, then the agent's entry, then the request; a fault in the entry is `AppError::Internal`, a refused request `AppError::Invalid` (a 400) |

`apply` runs `transition` on the thread's snapshot (state and job) and commits both together, writing the job only when it changed. The gate's `Watch`, `Schedule` and `RequestVerification` commands are not executed yet (they need the inbox, slice 5, and the verifier path, slice 10): they are logged at `warn` and dropped, and with the default gate the core never produces them. `submit`, the entry point for a user's request, refuses the machine inputs (`CiReported`, `VerifierReported`, `TimerFired`) with `AppError::Invalid`, so a user cannot forge a check result. |
| `NewThread`, `Inbound` (`message_id`, `run_id`, `key` a surface records in the log, and `gate`, the layer the request asks for; it applies when the request creates the thread), `Creation` (`Created` / `Exists`), `ApplyOutcome` (`Applied` / `Duplicate` / `Fenced`) | request and result types |
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
* `tests/gate_config.rs`: the layers (`GateRules`): `ci` and `verifier` are refused in every layer with the slice named, a layer adds sources and changes attempts within the cap, cannot remove a required source or choose the verifier per thread, is read strictly, the verifier is another configured agent, startup validation of every agent, and the thread created under deployment, entry and request (a refused request is a 400 and writes nothing).
* `tests/gate.rs`: the gate through the thread service: the job is created with the thread and saved with every change, a failed check reworks (a second delegation with the findings) and running out of attempts fails the thread, a thread keeps the gate it was created under when another replica is configured differently, `verifying` can be left by a CI report or a cancel, a user cannot submit a CI report, and a crash after a rework followed by a replay of the old task's envelopes (`completed` included) writes nothing.
* `tests/dispatcher.rs`: the dispatcher against the scripted agent, including a late result after another claimer (or the same owner name) re-claimed the row, with a heartbeat that never fires so only the fence can stop the old worker. Each claimed row is processed inside an `outbox` span (`id`, `thread`, `kind`, `attempt`), so every log line of that work carries them.
* `tests/a2ui.rs`: a surface is recorded before the question it accompanies, an action answers it and reaches the agent as an action on the same task, a replayed action is written once, an oversized action writes nothing, the action row is a delegation with its own payload.
* `tests/restart.rs`: two app instances over one shared in-memory "database",
  the first killed mid-stream.

Against real HTTP and Postgres, see [`orch-e2e`](../e2e/README.md).

## See also

[`orch-ports`](../ports/README.md), [`orch-api`](../api/README.md),
[`orch-e2e`](../e2e/README.md).
