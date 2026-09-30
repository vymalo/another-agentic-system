# orch-surface-agui

The AG-UI interaction surface. `POST /agui/agents/{agentId}` takes an AG-UI 1.0 `RunAgentInput` and
answers a stream of AG-UI events, projected from the event log for the requester.
`GET /agui/threads/{threadId}/connect` is the viewer's stream of a thread: replayed from the start or
from a `Last-Event-ID` cursor, then followed across runs. `GET /agui/agents/{agentId}/capabilities` is
the agent's `AgentCapabilities` document. The binding, with the mapping tables and the refusals, is
[`docs/api/agui.md`](../../../docs/api/agui.md)
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)).

## Where it sits

An adapter over [`orch-app`](../app/README.md)'s `App`, like every inbound surface
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)). It depends on
`App`, [`orch-core`](../core/README.md), `orch-ports` (for the `Ports` bound), the shared HTTP pieces
of [`orch-api`](../api/README.md) (problems, `SurfaceRoutes`, the SSE helpers) and the two pure AG-UI
crates: [`orch-agui-proto`](../agui-proto/README.md) (the wire types) and
[`orch-agui-projection`](../agui-projection/README.md) (`Projector`, `translate`). It decides
nothing about the frames: it reads the log, feeds the projector and writes what comes out. It is a
Cargo feature of the binary ([`orchestrator`](../../bin/orchestrator/README.md), feature
`surface-agui`, on by default) and is mounted by `ORCH_SURFACES` (name `agui`).

## API at a glance

| Item | What |
|---|---|
| `routes::<P>(Arc<App<P>>, sse_keepalive: Duration) -> orch_api::SurfaceRoutes` | the run route and the connect stream (streaming routes, no request timeout) and the capabilities document (an ordinary route), ready for `orch_api::router_with_surfaces` |
| `MAX_BODY_BYTES` | 8 MiB: what a request may weigh |

Mounted by `orch-api`, the route sits behind the identity layer like every route.

### One run request

1. **Headers and body.** `Content-Type: application/json` (415), `Accept` admits `text/event-stream` or
   is absent (406), at most 8 MiB (413), a `RunAgentInput` (400; members the schema does not declare are
   dropped with a warning), ids of at most 256 bytes.
2. **Thread.** `threadId` is a UUID the consumer minted (400 otherwise). The thread is the caller's, or
   free, or someone else's (404, the same answer as for a thread that does not exist for the caller, so a
   collision reveals nothing). The `agentId` of the URL exists (404) and is the thread's (409).
3. **Translate.** The log is folded into a `Projector`, and `orch_agui_projection::translate` reconciles
   the transcript by message id, reads `resume`, and returns one core input, or none (attach), or a
   refusal (400, 409, 422). Warnings (ignored `tools`, `context`, non-text parts, …) are logged.
4. **Apply.** A new thread is created with the consumer's id (`App::create_thread_as`, the release from
   `forwardedProps[<release-channels URI>].release`, validated against the live card); otherwise the
   input goes through `App::submit`. The event carries the message id and run id of the request and the
   idempotency key `agui:<threadId>:msg:<messageId>` (`agui:<threadId>:run:<runId>` for an answer with
   no message id). A concurrent request with the same ids loses the race and attaches. The gate a run
   asks for, `forwardedProps["vymalo.gate"]`, is read before anything else (a malformed one is a 400 whatever
   the thread) and applies when the run creates the thread: `App` resolves it on top of the deployment's and the
   agent's and refuses (400, before the stream) what weakens the gate or this build cannot honour. A run that continues a thread (or loses the race to create it) and asks for a gate different from the thread's is refused too (409, `App::gate_request_changes`); the same gate, or none, is served.
5. **Stream.** From the first event the input caused (or, for an attach, from that run's `RUN_STARTED`)
   to the first terminal event of the run: `RUN_FINISHED` or `RUN_ERROR`, then EOF. Frames carry `id:
   <seq>` on resume points. Keepalive comments every `sse_keepalive`.

A run is not tied to its connection: dropping the response never cancels; the cancel endpoint of the
resource API does, and the outcome arrives as `RUN_FINISHED` with outcome `cancelled`.

### One connect request

1. **Parameters.** `Accept` admits `text/event-stream` or is absent (406); `Last-Event-ID` is a
   non-negative integer, or absent or empty (400 otherwise); `?mode` is absent or `run` (400).
2. **Thread.** `parse_thread_id` and `App::get_thread`: a thread that does not exist for the caller, a
   malformed id and someone else's thread are the same 404 problem, before any stream byte.
3. **Stream.** `App::event_stream(user, thread, 0)` reads the log from the first event and then follows
   it (wakeups, with a poll under them), so the same code serves a replay, a cursor and the live tail
   on any replica. [`orch_agui_projection::Connect`](../agui-projection/README.md) folds the events,
   drops the frames up to the cursor, writes the preamble at the cursor when a run is open there, and
   says when a `?mode=run` stream is over. Frames carry `id: <seq>` on resume points; keepalive
   comments every `sse_keepalive`. The stream ends when the client closes, when `Connect` says so, or
   when the process shuts down and the stream has caught up (a truncated stream: the client reconnects
   with its cursor).

The connect handler decides nothing about the frames and keeps nothing between requests: no registry
of connections or runs, so a reconnect to another replica needs no shared memory. Dropping the
connection never cancels a run.

### The capabilities request

`App::describe_agent` reads the agent's card live (bounded by `AppConfig::card_timeout`, never
cached) and `orch_agui_projection::agent_capabilities` builds the document; the answer is
`application/json` with `Cache-Control: no-store`. An unreadable card gives the smaller document;
an unknown agent is a 404 problem.

## Features and environment

No Cargo features, no environment variables.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`) and the scripted agent, over real
HTTP; the harness in `tests/support` mounts this surface on `orch_api::router_with_surfaces`. Every
event the route emits is validated against the vendored AG-UI schema
(`orch_agui_proto::testkit::assert_json_conforms`). The dev-dependencies `jsonschema` and `serde_norway`
serve `tests/contract.rs`.

- `src/stream.rs` (unit): where a response starts (a run that opened meanwhile is opened again for the
  reader; frames before the start are folded and not written; an attach) and where it ends.
- `tests/runs.rs`: a new thread streamed and ended, ask and `resume`, an answer without `resume`, a
  failed task, cancel (`RUN_FINISHED` cancelled) through the endpoint and through `resume`, a dropped
  response not cancelling, keepalive, release through `forwardedProps`, ignored members, a 2 MiB body.
- `tests/attach.rs`: a retried POST replays the same frames, one while the run is going follows it to
  its end, earlier runs can be attached to, two concurrent requests with the same ids write one message,
  a retried answer.
- `tests/refusals.rs`: every status of the table in `docs/api/agui.md` (400, 401, 404, 406, 409, 413,
  415, 422, 502) as a problem, with nothing written; another owner's thread id; a message on a finished
  thread is served as the next job (and its retry is an attach, not a third job), a run while another is open is a 409.
- `tests/connect.rs`: a finished thread replayed and left open (only keepalives while idle), runs that
  come later followed, several viewers each getting the whole stream, a reconnect in the middle of a run
  (the preamble, then the rest once), a reconnect from every resume point of a two-run thread, a cursor
  at or beyond the end, `?mode=run` on an idle thread and on a running one, a closed connection not
  cancelling, the stream ending at shutdown, every refusal (404 for missing, malformed and foreign
  threads with one body, 401, 400, 406).
- `tests/capabilities.rs`: the document conforms and describes the agent, release channels are declared
  only while the live card lists them, 404 and 401.
- `tests/a2ui.rs`: a surface reaches the requester and a later viewer whole; an action is delivered to the same task and answers the wait; an action for an unknown surface, on a new thread, malformed, oversized (413), beside a message, on someone else's thread, on another agent's thread, on a finished thread, or under a reused run id is refused before the stream with nothing written or sent; the capabilities document declares A2UI only while the live card lists it (each URI, both, card down, card changed).
- `tests/contract.rs`: `docs/api/chat-api.yaml` against this surface. It drives `runAgent`,
  `connectThread` and `getAgentCapabilities` and fails when the statuses the contract documents differ
  from the ones answered (one named exemption: a store that fails to read, the 503 of `connectThread`),
  validates every problem, capabilities document and stream frame against the contract's schemas (which
  reference the vendored AG-UI schema by file; the test checks the reference resolves to it), and
  that its validator bites. (The resource API is covered by
  [`orch-api`](../api/README.md)'s `tests/contract.rs`.)

Against the fake A2A agent and Postgres, see [`orch-e2e`](../e2e/README.md) (`agui_run.rs`, which also
writes the run goldens `docs/api/examples/agui/run-*.agui.json`; `agui_connect.rs`, with the
killed-replica reconnect, which writes the connect and capabilities goldens).

## See also

[`orch-agui-projection`](../agui-projection/README.md), [`orch-agui-proto`](../agui-proto/README.md),
[`orch-api`](../api/README.md), [`orch-app`](../app/README.md).
