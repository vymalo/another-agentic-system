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
| `Input` | `UserMessage { user, text, message_id?, run_id? }` (the ids a surface such as AG-UI names are recorded in the log), `UiAction { user, action }` (the user acted on an A2UI surface; answers a blocked thread like a message), `Cancel`, `Agent { agent, revision, update }`, `DeliveryFailed { reason, retryable }`, `CancelledBeforeStart` |
| `Command` | `Append(EventDraft)`, `Delegate { text }`, `DelegateAction { action }`, `RequestCancel` |
| `Event`, `EventKind`, `EventBody`, `Actor`, `ActorType` and the `*Data` payloads | the append-only event log the chat renders |
| `AgentUpdate`, `AgentTaskState` | the protocol-neutral update an agent adapter produces: status, artifact, message, and the A2UI pair `Ui { operations }` (a payload that passed the envelope check) and `UiRejected { reason }` (one that did not: an `error` event, nothing of it passed on) |
| `check_operations`, `inspect`, `UiRejection`, `UiVersion`, `SurfaceOp`, `UiSurfaceData`, `UiActionData`, `UiActionError`, the `MAX_*` limits, `A2UI_EXTENSION_V0_9_1`, `A2UI_EXTENSION_V1_0`, `A2UI_MEDIA_TYPE` | A2UI in the core ([ADR 0013](../../../docs/decisions/0013-a2ui-generative-ui.md)): the envelope every agent payload passes (the A2A adapter checks each part, and `transition` checks every `AgentUpdate::Ui` again, turning an unchecked one into an `error` event; a JSON array of at most 256 messages and 64 KiB, each with a known `version` and exactly one of the four surface operations naming a `surfaceId`), the shape and size rules of a user's action, the extension URIs and the catalog ids of the web renderer. Pure functions over `serde_json::Value`; components are not validated here |
| `ThreadRecord`, `AgentInfo`, `AgentTarget`, `Releases` | thread and agent descriptions |
| `AgentId`, `ThreadId`, `UserId`, `Timestamp` | ids and time (`jiff`, no `f64` time) |
| `Classify`, `ErrorClass`, `BoxError`, `report` | one classification model for every error: retry, HTTP status and exit-code decisions match on `ErrorClass` (`Transient`, `RateLimited`, `Conflict`, `Invalid`, `NotFound`, `Rejected`), never on variants |

```rust
use orch_core::{transition, Input, ThreadState, UserId};

// A follow-up in a blocked thread re-queues it and appends the user's message.
let (next, commands) = transition(
    &ThreadState::Blocked,
    &Input::UserMessage {
        user: UserId::new("me@example.com"),
        text: "main".into(),
        message_id: None,
        run_id: None,
    },
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
* `tests/wire.rs`: the JSON must match the contract schemas exactly, including the additive kinds `ui_surface` and `ui_action`.
* `src/ui.rs` (unit): the envelope rules one by one, the caps at their edge, that a refusal repeats at most an excerpt, the action checks.

## See also

[`orch-ports`](../ports/README.md), [`orch-app`](../app/README.md).
