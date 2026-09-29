# orch-core

The pure core of the orchestrator: the contract types of the chat API and the
one function that decides what happens next, `transition`. No async, no I/O,
no protocol dependencies.

## Where it sits

The innermost crate. Dependency direction: `core` <- `ports` <- `app` <- `api`
([workspace overview](../../README.md#crates)). Nothing here talks to a store,
an agent or the network, so the compiler enforces that the core stays pure
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md),
[ADR 0004](../../../docs/decisions/0004-closed-enums-over-dyn-registry.md):
protocols are closed enums). The JSON shapes match
[`docs/api/chat-api.yaml`](../../../docs/api/chat-api.yaml); the design is in
[`docs/orchestrator.md`](../../../docs/orchestrator.md).

## API at a glance

| Item | What |
|---|---|
| `transition(&ThreadState, &Input) -> Result<(ThreadState, Vec<Command>), TransitionError>` | the pure decision function |
| `ThreadState` | `Queued`, `Working`, `Blocked`, `Done`, `Failed`, `Cancelled` |
| `Input` | `UserMessage`, `Cancel`, `Agent { agent, revision, update }`, `DeliveryFailed { reason, retryable }`, `CancelledBeforeStart` |
| `Command` | `Append(EventDraft)`, `Delegate { text }`, `RequestCancel` |
| `Event`, `EventKind`, `EventBody`, `Actor`, `ActorType` and the `*Data` payloads | the append-only event log the chat renders |
| `AgentUpdate`, `AgentTaskState` | the protocol-neutral update an agent adapter produces |
| `ThreadRecord`, `AgentInfo`, `AgentTarget`, `Releases` | thread and agent descriptions |
| `AgentId`, `ThreadId`, `UserId`, `Timestamp` | ids and time (`jiff`, no `f64` time) |
| `Classify`, `ErrorClass`, `BoxError`, `report` | one classification model for every error: retry, HTTP status and exit-code decisions match on `ErrorClass` (`Transient`, `RateLimited`, `Conflict`, `Invalid`, `NotFound`, `Rejected`), never on variants |

```rust
use orch_core::{transition, Input, ThreadState, UserId};

// A follow-up in a blocked thread re-queues it and appends the user's message.
let (next, commands) = transition(
    &ThreadState::Blocked,
    &Input::UserMessage { user: UserId::new("me@example.com"), text: "main".into() },
)?;
assert_eq!(next, ThreadState::Queued);
```

## Features and environment

None.

## Tests

Offline, no environment variables.

* `tests/transition_table.rs`: one test per row of the transition table
  ([`docs/orchestrator.md`](../../../docs/orchestrator.md)).
* `tests/properties.rs`: `proptest` properties over random input sequences.
* `tests/wire.rs`: the JSON must match the contract schemas exactly.

## See also

[`orch-ports`](../ports/README.md), [`orch-app`](../app/README.md).
