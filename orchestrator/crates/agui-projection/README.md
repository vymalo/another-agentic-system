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
orchestrator: no `orch-app`, no store, no HTTP. The AG-UI surface ([`orch-surface-agui`](../surface-agui/README.md)) is the adapter that feeds it events from `App::event_stream` and writes its frames as SSE.

## API at a glance

| Item | What |
|---|---|
| `Projector::new(ThreadMeta)` | the projection of one thread, before its first event. `ThreadMeta` is what `STATE_SNAPSHOT` shows besides the state: thread id, title, target, and the `gate` of the thread's job (`GatePolicy`, fixed when the thread was created; the default requires nothing) |
| `Projector::apply(&Event, Audience) -> Vec<Frame>` | folds the next log event in; one call per event, in `seq` order |
| `Frame { event, resume_id }` | an AG-UI event and, on the last frame of a log event with no text message open, the SSE `id:` (`Some(seq)`) a client may resume from |
| `Audience::{Viewer, Requester { held_message_ids }}` | the connect stream gets everything; the POST that sent the input skips the user messages it already holds (`held_message_ids(&input)`) |
| `Projector::resume_preamble() -> Vec<Frame>` | re-opens the current run (same `RUN_STARTED`, the open `SUBAGENT_STARTED`, a `STATE_SNAPSHOT`) for a client that reconnects mid-run; empty between runs |
| `Connect::new(ThreadMeta, cursor, head, Follow)`, `feed(&Event) -> Vec<Frame>`, `finished()` | the connect stream as a fold: the events up to the cursor are folded and not written, the preamble comes at the cursor, every later event is written; `Follow::ThroughRun` (`?mode=run`) is `finished()` once the log as it stood at `head` is replayed and no run is open. A cursor beyond `head` is clamped to it |
| `agent_capabilities(&AgentId, name, Option<&CardFacts>) -> AgentCapabilities` | the capabilities document from what the live card says (`CardFacts.ui`: the A2UI versions it lists, declared under each extension URI with `supportedCatalogIds`); `None` (an unreadable card) gives the smaller one |
| `ui_surface` / `ui_action` | A2UI ([ADR 0013](../../../docs/decisions/0013-a2ui-generative-ui.md)): the projector keeps the operations of every live surface and sends each touched surface **whole** as `ACTIVITY_SNAPSHOT{activityType:"a2ui-surface", replace:true, content:{a2ui_operations}}` under the message id `a2ui-<seq>` of its first event (capped at 256 KiB of operations; a delete ends the surface; an operation that fails the envelope check is skipped, never relayed); a `ui_action` is a `vymalo.action` activity that opens a run under the action's `runId`. `ACTIVITY_A2UI_SURFACE`, `ACTIVITY_ACTION`, `A2UI_OPERATIONS_KEY` |
| `Projector::view(&UserId) -> ThreadView` | what the thread holds, for `translate` |
| `check_result` / `rework` / the gate | the verification gate ([ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md), [`agui.md`](../../../docs/api/agui.md#verification-the-gate)). Under a gate that requires something the run stays open while the thread is `queued`, `working` or `verifying`. `agent_status{completed}` is `SUBAGENT_FINISHED` and a `STATE_SNAPSHOT` with `thread.state: "verifying"` and `job {attempt, maxAttempts, gate, sha?}` (every `STATE_SNAPSHOT` has `job` under a gate, none without one); `check_result` is the `ACTIVITY_SNAPSHOT` `vymalo.check` (`ACTIVITY_CHECK`, id `check-<attempt>-<source>`, `replace: true`; a `stale` answer is its own card `evt-<seq>` and changes nothing else); `rework` is `vymalo.rework` (`ACTIVITY_REWORK`, id `rework-<attempt>`), then `SUBAGENT_STARTED` for the next attempt and a `STATE_SNAPSHOT` (`queued`); `thread_state{done}` is `RUN_FINISHED` success, and the `error` that follows a failed check with `thread_state{failed}` is `RUN_ERROR` with `code: "checks_failed"` (`CODE_CHECKS_FAILED`). `ci_result` is not projected yet (slice 7) |
| `translate(&RunAgentInput, &ThreadView) -> Result<Vec<Input>, InputError>` | new user message, `resume` answer or cancel, `forwardedProps.a2uiAction.userAction` (shape, size, and that `ThreadView`'s surfaces include the one named: `Input::UiAction` with the surface's version and the request's run id), or attach; `translate_with_warnings` also returns what was ignored |
| `InputError::http_status()` | the status (400, 409, 413, 422) of a request refused before the stream (including a `runId` reused for new input; 413 is an A2UI action over the limits) |
| `thread_id_of`, `release_selector`, `held_message_ids` | the request members a surface reads itself |

```rust
use orch_agui_projection::{Audience, Connect, Follow, Projector, ThreadMeta};

let mut projector = Projector::new(meta);
for event in log {
    for frame in projector.apply(&event, Audience::Viewer) {
        // write `frame.event` as one SSE `data:` line, and `id: <seq>` when `frame.resume_id` is set
    }
}
// A client reconnects with Last-Event-ID = c: fold the events up to c, discard their frames,
// send `resume_preamble()`, then keep applying the events after c. `Connect` is that, packaged:
let mut connect = Connect::new(meta, cursor, head, Follow::Forever);
for event in log {
    for frame in connect.feed(&event) { /* write it */ }
    if connect.finished() { break; } // only with Follow::ThroughRun
}
```

## Rules the fold keeps

- **A function of the events.** The state never depends on the frames emitted or on the
  audience, so every replica agrees and a reconnect can rebuild it by folding up to the cursor.
- **Runs are balanced.** A run opens at the first event of a burst of activity (a user message, or
  an event nobody asked for) and closes at the event that ends it; nothing a run opened (a text
  message, a subagent invocation) is open when it ends. Between the core's transactions a run is
  open exactly when the thread is `queued`, `working` or `verifying`. The module docs of `src/projector.rs`
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
- `tests/verify.rs`: the gate on hand-made logs: the run stays open while verifying and a client that joins then is told where the job stands, one run and two subagents across a rework, the check and rework cards and their ids, `checks_failed`, a delivery failure after a rework, a job without a gate is unchanged (no `job` anywhere), a stale answer.
- `tests/props.rs` (proptest over logs made by the core's own `transition`, with and without a gate): the stream is well
  formed at every prefix (against a Rust model of the reference consumer's rules in
  `tests/support/verify.rs`, and the vendored schema through `orch_agui_proto::testkit`); nothing
  is open at a terminal event; resuming from any resume point gives exactly the suffix, and the
  preamble plus the suffix is a well-formed stream; the requester differs from the viewer only in
  the messages it holds.
- `tests/connect.rs`: `Connect` over the same random logs. From every cursor the stream is the preamble of
  the state at the cursor followed by the frames of every later event, which is what an uninterrupted
  stream wrote after that `id:` (the frames a client held plus the reconnect are the stream once); the
  result is well formed; `?mode=run` writes what the default writes and ends at the first idle point at
  or after the end of the log as it stood at connect time, for every head and cursor; the edges by hand
  (a cursor beyond the head, the preamble at the cursor before the next event, an idle thread, a gap).
- `tests/capabilities.rs`: the document with and without release channels, with each A2UI extension URI and both, and for an unreadable card,
  validated against the schema.
- `tests/a2ui.rs`: surfaces and actions on hand-written logs: whole-surface snapshots under one message id, two surfaces in one payload, delete and recreate, the replay cap, foreign operations skipped, a late surface, an action's run and activity, resuming from a cursor. `tests/props.rs` and `tests/connect.rs` include surfaces, refused parts and actions in their random logs.
- `tests/translate.rs`: every row of the inbound table, and that re-sending the whole transcript
  never duplicates input; the A2UI action rows (the surface the thread has and its version, unknown or deleted surface, malformed, oversized, ambiguous, the rules of any input).
- `tests/golden.rs`: [`docs/api/examples/agui/*.agui.json`](../../../docs/api/examples/README.md) are
  produced from the `*.events.json` goldens; every frame conforms to the schema and the streams are
  well formed. `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test golden` rewrites them.
  `tools/agui-conformance` feeds the same files through the reference client in CI.

## See also

[`orch-agui-proto`](../agui-proto/README.md), [`orch-core`](../core/README.md),
[`docs/api/agui.md`](../../../docs/api/agui.md).
