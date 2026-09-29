# orch-surface-agui

The AG-UI interaction surface: `POST /agui/agents/{agentId}` takes an AG-UI 1.0 `RunAgentInput` and
answers a stream of AG-UI events, projected from the event log for the requester. The binding, with
the mapping tables and the refusals, is [`docs/api/agui.md`](../../../docs/api/agui.md)
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)). The connect stream
(`GET /agui/threads/{id}/connect`) and the capabilities document are later slices.

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
| `routes::<P>(Arc<App<P>>, sse_keepalive: Duration) -> orch_api::SurfaceRoutes` | the run route, a streaming route (no request timeout), ready for `orch_api::router_with_surfaces` |
| `MAX_BODY_BYTES` | 8 MiB: what a request may weigh |

Mounted by `orch-api`, the route sits behind the identity layer like every route.

### One request

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
   no message id). A concurrent request with the same ids loses the race and attaches.
5. **Stream.** From the first event the input caused (or, for an attach, from that run's `RUN_STARTED`)
   to the first terminal event of the run: `RUN_FINISHED` or `RUN_ERROR`, then EOF. Frames carry `id:
   <seq>` on resume points. Keepalive comments every `sse_keepalive`.

A run is not tied to its connection: dropping the response never cancels; the cancel endpoint of the
resource API does, and the outcome arrives as `RUN_FINISHED` with outcome `cancelled`.

## Features and environment

No Cargo features, no environment variables.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`) and the scripted agent, over real
HTTP; the harness in `tests/support` mounts this surface on `orch_api::router_with_surfaces`. Every
event the route emits is validated against the vendored AG-UI schema
(`orch_agui_proto::testkit::assert_json_conforms`).

- `src/stream.rs` (unit): where a response starts (a run that opened meanwhile is opened again for the
  reader; frames before the start are folded and not written; an attach) and where it ends.
- `tests/runs.rs`: a new thread streamed and ended, ask and `resume`, an answer without `resume`, a
  failed task, cancel (`RUN_FINISHED` cancelled) through the endpoint and through `resume`, a dropped
  response not cancelling, keepalive, release through `forwardedProps`, ignored members, a 2 MiB body.
- `tests/attach.rs`: a retried POST replays the same frames, one while the run is going follows it to
  its end, earlier runs can be attached to, two concurrent requests with the same ids write one message,
  a retried answer.
- `tests/refusals.rs`: every status of the table in `docs/api/agui.md` (400, 401, 404, 406, 409, 413,
  415, 422, 502) as a problem, with nothing written; another owner's thread id.

Against the fake A2A agent and Postgres, see [`orch-e2e`](../e2e/README.md) (`agui_run.rs`, which also
writes the run goldens `docs/api/examples/agui/run-*.agui.json`).

## See also

[`orch-agui-projection`](../agui-projection/README.md), [`orch-agui-proto`](../agui-proto/README.md),
[`orch-api`](../api/README.md), [`orch-app`](../app/README.md),
[`orch-surface-chat-api`](../surface-chat-api/README.md).
