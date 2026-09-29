# orch-agui-projection

The AG-UI view of the event log, as pure functions: a fold from orchestrator events to AG-UI
frames, and the translation of a `RunAgentInput` to core inputs. No `async`, no I/O, so the
compiler enforces that the projection stays a function of the log.

The normative mapping is [`docs/api/agui.md`](../../../docs/api/agui.md)
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)): the event log is the only
source of truth ([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)) and AG-UI
is a view of it. Protocols are closed enums
([ADR 0004](../../../docs/decisions/0004-closed-enums-over-dyn-registry.md)), so the frames are the
typed events of [`orch-agui-proto`](../agui-proto/README.md).

## Where it sits

Depends on [`orch-core`](../core/README.md) (the events and inputs) and
[`orch-agui-proto`](../agui-proto/README.md) (the wire types), and on nothing else of the
orchestrator: no `orch-app`, no store, no HTTP. The AG-UI surface (`orch-surface-agui`, a later
slice) is the adapter that feeds it events from `App::event_stream` and writes its frames as SSE.

## API at a glance

| Item | What |
|---|---|
| `Projector::new(ThreadMeta)` | the projection of one thread, before its first event. `ThreadMeta` is what `STATE_SNAPSHOT` shows besides the state: thread id, title, target |
| `Projector::apply(&Event, Audience) -> Vec<Frame>` | folds the next log event in; one call per event, in `seq` order |
| `Frame { event, resume_id }` | an AG-UI event and, on the last frame of a log event with no text message open, the SSE `id:` (`Some(seq)`) a client may resume from |
| `Audience::{Viewer, Requester { held_message_ids }}` | the connect stream gets everything; the POST that sent the input skips the user messages it already holds (`held_message_ids(&input)`) |
| `Projector::resume_preamble() -> Vec<Frame>` | re-opens the current run (same `RUN_STARTED`, the open `SUBAGENT_STARTED`, a `STATE_SNAPSHOT`) for a client that reconnects mid-run; empty between runs |
| `Projector::view(&UserId) -> ThreadView` | what the thread holds, for `translate` |
| `translate(&RunAgentInput, &ThreadView) -> Result<Vec<Input>, InputError>` | new user message, `resume` answer or cancel, or attach; `translate_with_warnings` also returns what was ignored |
| `InputError::http_status()` | the status (400, 409, 422) of a request refused before the stream |
| `thread_id_of`, `release_selector`, `held_message_ids` | the request members a surface reads itself |

```rust
use orch_agui_projection::{Audience, Projector, ThreadMeta};

let mut projector = Projector::new(meta);
for event in log {
    for frame in projector.apply(&event, Audience::Viewer) {
        // write `frame.event` as one SSE `data:` line, and `id: <seq>` when `frame.resume_id` is set
    }
}
// A client reconnects with Last-Event-ID = c: fold the events up to c, discard their frames,
// send `resume_preamble()`, then keep applying the events after c.
```

## Rules the fold keeps

- **A function of the events.** The state never depends on the frames emitted or on the
  audience, so every replica agrees and a reconnect can rebuild it by folding up to the cursor.
- **Runs are balanced.** A run opens at the first event of a burst of activity (a user message, or
  an event nobody asked for) and closes at the event that ends it; nothing a run opened (a text
  message, a subagent invocation) is open when it ends. Between the core's transactions a run is
  open exactly when the thread is `queued` or `working`. The module docs of `src/projector.rs`
  explain how a run closes, including the two cases the log cannot tell apart.
- **Ids come from the log.** `run-<seq>`, `sub-<seq>`, `int-<seq>`, `evt-<seq>`, or the ids a
  surface recorded (`user_message.data.messageId` / `runId`).
- **Resume points** are the sequence numbers of whole log events, and never fall inside a text
  message.

## Features and environment

None, and no environment variables.

## Tests

Offline, no database. `cargo test -p orch-agui-projection`:

- `tests/projection.rs`: every row of the outbound table on a hand-written log (asking and
  answering, `auth_required`, failure, cancel, delivery failures, late and repeated events, partial
  messages, audiences, the resume preamble, the view).
- `tests/props.rs` (proptest over logs made by the core's own `transition`): the stream is well
  formed at every prefix (against a Rust model of the reference consumer's rules in
  `tests/support/verify.rs`, and the vendored schema through `orch_agui_proto::testkit`); nothing
  is open at a terminal event; resuming from any resume point gives exactly the suffix, and the
  preamble plus the suffix is a well-formed stream; the requester differs from the viewer only in
  the messages it holds.
- `tests/translate.rs`: every row of the inbound table, and that re-sending the whole transcript
  never duplicates input.
- `tests/golden.rs`: [`docs/api/examples/agui/*.agui.json`](../../../docs/api/examples/README.md) are
  produced from the `*.events.json` goldens; every frame conforms to the schema and the streams are
  well formed. `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test golden` rewrites them.
  `tools/agui-conformance` feeds the same files through the reference client in CI.

## See also

[`orch-agui-proto`](../agui-proto/README.md), [`orch-core`](../core/README.md),
[`docs/api/agui.md`](../../../docs/api/agui.md).
