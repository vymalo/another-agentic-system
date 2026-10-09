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
| `PROJECTION_VERSION` | re-exported from [`orch-agui-projection`](../agui-projection/README.md): the version of the frames this build writes (`ui.history.projection` of `GET /api/config`, `projection` of a page) |
| The history routes | `GET /agui/threads/{threadId}/history` (ordinary request, behind the identity layer, `thread.read`), `GET /agui/shared/{token}/history` (signed in) and `GET /agui/public/shared/{token}/history` (`SurfaceRoutes::public`: rate limited, **one stream permit held while it folds**, `429` with `Retry-After` and `code: too_many_streams`): a finite page of the connect stream's frames, `before` / `limit` / `since` / `after`, as one JSON document ([`history.md`](../../../docs/api/history.md), [ADR 0059](../../../docs/decisions/0059-a-thread-opens-at-its-end-and-older-turns-load-on-scroll-up.md)) |
| The shared connect routes | `GET /agui/shared/{token}/connect` (streaming, signed in, `thread.read`) and `GET /agui/public/shared/{token}/connect` (`SurfaceRoutes::public`: outside the identity layer, rate limited, one permit per open stream, `429` with `Retry-After` and `code: too_many_streams` when the link's streams or all links' are taken). See *A shared thread's connect request* |
| Live text | both streams read `App::thread_feed` (the log with the live text of the thread's replies mixed in, [ADR 0027](../../../docs/decisions/0027-live-text-relayed-not-stored.md)) and pass what they hear through the connection's own `LiveOverlay` ([`orch-agui-projection`](../agui-projection/README.md)): the log's frames go through `overlay.logged` (the final message of a live message continues it), a piece through `overlay.live`, **only when the stream is caught up** (the connect stream: `Connect::caught_up`, the log folded up to the head at connect time; the run response: the run is being written). Live frames carry no `id:`; a new connection starts with an empty overlay and is told the text so far by the sender's refresh |

Mounted by `orch-api`, the route sits behind the identity layer like every route.

### One run request

1. **Headers and body.** `Content-Type: application/json` (415), `Accept` admits `text/event-stream` or
   is absent (406), at most 8 MiB (413), a `RunAgentInput` (400; members the schema does not declare are
   dropped with a warning), ids of at most 256 bytes.
2. **Thread.** `threadId` is a UUID the consumer minted (400 otherwise). The thread is the caller's, or
   free, or someone else's (404, the same answer as for a thread that does not exist for the caller, so a
   collision reveals nothing; nobody reads or continues another person's thread, an administrator included: 404, [ADR 0039](../../../docs/decisions/0039-nobody-reads-another-persons-thread.md)). The caller needs `thread.write` and `agent.invoke` for the agent (403, `forbidden`, [ADR 0033](../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)). The `agentId` of the URL exists (404) and is the thread's (409).
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
   A message sent **while a run is open** is served when it says how, `forwardedProps["vymalo.send"]` (`"steer"` or `"interrupt"`, [ADR 0036](../../../docs/decisions/0036-sending-while-an-agent-works.md), [`agui.md`](../../../docs/api/agui.md#sending-while-an-agent-works)): `interrupt` becomes `Input::StopAndSend`, and the response starts at the `RUN_STARTED` of the run the message opened (`Start::Run`), because the event that carries the message finishes the run that was open. Without the member it is the 409 it was; any other value is a 400. The MCP servers to attach, `forwardedProps["vymalo.tools"]` ([ADR 0024](../../../docs/decisions/0024-mcp-tools-attached-per-conversation.md)), an array of server ids, are read on every run too (**400** before the stream when it is not an array of strings), applied only when the run creates the thread (`Inbound.tools`: the creation commit holds the message, then `tools_attached`; **422** for a server the deployment does not offer for the agent, or more than 16, with nothing created) and ignored, with a warning, on a run that continues a thread (the API's `PUT` is the door).
   The screen's UI catalog, `forwardedProps["vymalo.uiCatalog"]` ([ADR 0023](../../../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md)), is read on every run too and refused before the stream when it breaks a rule: the envelope (`UiCatalogData::from_json`: 400, **413** over 64 KiB; the digest is recomputed) and the schemas (`orch_app::check_catalog_schemas`: 400); nothing is written. It is applied only when the run applies an input: it goes to the first message or action as its `catalog` (a new thread through `Inbound.ui_catalog`), and a run that attaches, or only stops, ignores it. `docs/api/agui.md`, "The UI catalog".
   A run that **creates its thread as a fork**, `forwardedProps["vymalo.fork"] = {from, after}` ([ADR 0042](../../../docs/decisions/0042-the-thread-list-is-the-owners.md), `fork_request` in `run.rs`), is read on every run (400 before the stream when it is not exactly a UUID and an integer, or comes with `vymalo.gate` or `vymalo.tools`); when the run creates the thread `App::fork_and_send` makes the fork and the run's message (its mentions and catalog too) in one transaction and the response starts at the `RUN_STARTED` of that run, the copied events folded and not written. A thread that exists already is the replay when it is that fork (`orch_core::is_fork_at` and the first message's `runId`): the run attaches and writes nothing; any other is a 409.
   The agents the message mentions, `forwardedProps["vymalo.mentions"]` ([ADR 0026](../../../docs/decisions/0026-agent-mentions-as-structured-references.md), [`mentions-v1.md`](../../../docs/api/mentions-v1.md)), are read on every run too (`orch_app::mentions::parse`: **400** before the stream when the member is not an array of at most 16 references of the right shape) and go with the run's one message, a stop included (`mentioning`, beside `carrying`, which gives the catalog to the first message or action); `App` checks them against the text, the registry and the person's roles before anything is written (**422**, **503**: see the table in `docs/api/agui.md`). On a run with no message to carry them the member is ignored with a warning.
5. **Stream.** From the first event the input caused (or, for an attach, from that run's `RUN_STARTED`)
   to the first terminal event of the run: `RUN_FINISHED` or `RUN_ERROR`, then EOF. Frames carry `id:
   <seq>` on resume points. Keepalive comments every `sse_keepalive`.

A run is not tied to its connection: dropping the response never cancels; the cancel endpoint of the
resource API does, and the outcome arrives as `RUN_FINISHED` with outcome `cancelled`.

### One connect request

1. **Parameters.** `Accept` admits `text/event-stream` or is absent (406); `Last-Event-ID` is a
   non-negative integer, or absent or empty (400 otherwise); `?mode` is absent or `run` (400).
2. **Thread.** `parse_thread_id` and `App::get_thread`: a thread that does not exist for the caller, a
   malformed id and a thread the caller may not read (someone else's, unless their roles read every thread) are the same 404 problem, before any stream byte; roles with no `thread.read` are 403. The stream is bounded by the token it was opened with: it ends at its `exp` plus 60 s, and after an hour at most (`orch_api::sse::bounded`), and the client reconnects with `Last-Event-ID`; a credential that does not run out is not bounded.
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

### A shared thread's connect request

([ADR 0040](../../../docs/decisions/0040-thread-sharing-by-revocable-link.md), [`agui.md`](../../../docs/api/agui.md#reading-a-shared-thread).) The same stream, read-only, for a person holding a link.

1. **Parameters** are checked as for a connect (406, 400) and the cursor is validated **before** a stream permit is taken.
2. **The link.** `App::open_shared` (or `open_public`): every link that does not work is the one 404 problem, before any stream byte.
3. **The permit** (public only) is taken after the link is known to work, so a guess takes none.
4. **Stream.** `App::shared_feed` and the reader projection (`orch_app::reader_event`): the owner is "the owner", hidden events are inert events with their `seq`, and a public reader gets no step input, output or detail (unless `sharing.public.stepIo`) and no files (unless `sharing.public.files`). The frames are the connect's, with the headers of every shared answer (`no-store, no-transform`, `noindex`). The stream ends when the link stops working: a revocation, a rotation or a lowered cap, rechecked on `thread_shared` and `thread_unshared` and every 30 s, and the reconnect is the 404.

There is no run route for a link: a reader sends nothing, and the thread's own routes stay its owner's.


### A history request

([ADR 0059](../../../docs/decisions/0059-a-thread-opens-at-its-end-and-older-turns-load-on-scroll-up.md), [`history.md`](../../../docs/api/history.md).) One finite answer: the page of the thread's frames that [`orch_agui_projection::History`](../agui-projection/README.md) folds.

1. **Parameters** are checked before anything is read: `Accept` must admit `application/json` (406), `before`, `since` and `after` are integers of 1 or more, `limit` is 1 to `server.history.maxTurns` (default `ui.history.pageTurns`), `limit`, `since` and `after` exclude each other and `before` does not go with `after` (400, with the parameter named).
2. **The thread.** `App::history_log` (the owner: 404 for a thread that is missing, malformed or someone else's, 403 for roles without `thread.read`) or `App::open_shared` / `open_public` (the link: the one 404). The public route takes its **stream permit** here, after the link is known to work, and holds it until the answer is built (or the 30 s read bound ends it with a 503): a request costs a fold, and the permit is what bounds how many run at once.
3. **The fold.** The log is read from the first event in pages of 500 (`App::history_log`; for a reader through `reader_event`) and handed to `History::feed` until it says `Flow::Done` (a read that goes back from `before` stops at the chain that holds it). A log that ends before the head the thread had (a delete meanwhile) is a 404.
4. **The answer** is `HistoryPage` of the contract: `Cache-Control: no-store`, and `X-Robots-Tag: noindex, nofollow` on the shared routes. `App::history_answered` counts it (`history_pages_total`, `history_events_folded_total`, `history_fold_seconds_total` at `/metrics`).

### The capabilities request

`App::describe_agent` reads the agent from the registry now (ADR 0022) and its card live (bounded by `AppConfig::card_timeout`, never
cached) and `orch_agui_projection::agent_capabilities` builds the document, which lists the A2UI extensions and the extensions of the orchestrator's own (`ui-catalog/v1`, …) the card lists under `custom`, so the web can flag an agent before it sends anything; the answer is
`application/json` with `Cache-Control: no-store`. An unreadable card gives the smaller document;
an agent the registry answers without is a 404 problem, and a registry that cannot say is a 503 ("the agent registry is unreachable"), as for a run: `App::resolve_agent` is how both ask.

## Features and environment

No Cargo features, no environment variables.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`) and the scripted agent, over real
HTTP; the harness in `tests/support` mounts this surface on `orch_api::router_with_surfaces`. Every
event the route emits is validated against the vendored AG-UI schema
(`orch_agui_proto::testkit::assert_json_conforms`). The dev-dependencies `jsonschema` and `serde_norway`
serve `tests/contract.rs`.

- `tests/shared.rs` (ADR 0040): a signed-in reader is replayed the thread without its owner; a reader follows what the owner writes after sharing; every refusal comes before the stream and a dead link is one 404; a reader can send nothing; anybody reads a public link and the identity they send is ignored; a revocation ends an open stream and the reconnect is a 404; public streams are held per link and in all. `tests/contract.rs` also drives the two shared operations (`connectSharedThread`, `connectPublicSharedThread`).
- `tests/live.rs`: live text on both streams, over the scripted agent's `stream*` scripts: the requester's response shows the reply growing and then completes it (one live message, the log's message with its resume point), a viewer sees live frames with no `id:` and the final with one and reads the reply once, a second viewer that joins while the agent is quiet reads it once by the refresh and the final, a connection opened after the reply is in the log sees the plain message with no live frame, and a reply given up ends marked on the screen and is not in the log.
- `src/stream.rs` (unit): where a response starts (a run that opened meanwhile is opened again for the
  reader; frames before the start are folded and not written; an attach) and where it ends.
- `tests/runs.rs`: a new thread streamed and ended, ask and `resume`, an answer without `resume`, a
  failed task, cancel (`RUN_FINISHED` cancelled) through the endpoint and through `resume`, a dropped
  response not cancelling, keepalive, release through `forwardedProps`, ignored members, a 2 MiB body.
- `tests/attach.rs`: a retried POST replays the same frames, one while the run is going follows it to
  its end, earlier runs can be attached to, two concurrent requests with the same ids write one message,
  a retried answer.
- `tests/tools.rs`: attached servers on a run (ADR 0024): a creating run records the message then `tools_attached` and says the set in every later snapshot and as a `vymalo.tools` card, the agent is told what is attached (no URL), an empty or `null` member attaches nothing, every malformed shape is a 400 and every server that cannot be attached a 422 with no thread made and nothing sent, a continuing run ignores the member (and a malformed one is still refused), a retry of the creating run records nothing twice, and a viewer that connects later is told the set.
- `tests/fork.rs`: a run that creates its thread as a fork (ADR 0042): no thread before the send, the fork made with its message in one commit (the copy, `thread_forked`, the message, `job_started`), the response starting at its own `RUN_STARTED` with none of the copied turns, the agent told the parent's transcript, mentions and catalog applied to the message, the resend attaching and writing no second message (also after the parent went on), another thread's id, another cut, run or message a 409 and somebody else's a 404, a parent that is not the caller's or missing a 404, an open turn a 409 `turn_open`, a cut outside the log a 422, a malformed member or one with a gate or tools a 400, a `null` member an ordinary thread. `tests/contract.rs` drives the same run and its refusals against the contract.
- `tests/mentions.rs` and `tests/roles.rs`: mentions on a run (ADR 0026): `forwardedProps["vymalo.mentions"]` is read on every run and refused before the stream when malformed (400: not an array, more than 16, a member missing, of the wrong type, or one a reference does not have), refused (422) when a label is not the text at its offsets (counted in code points or in bytes instead of UTF-16 units, inside a surrogate pair, past the end, overlapping or out of order), when the agent is unknown or its card moved, when it is the thread's own agent, or when the caller's role may not invoke it (`you may not use 'coder'`, before the registry is asked), and 503 with `Retry-After` while the registry cannot say, with no thread made and nothing sent in every case; a valid one is recorded as sent, shown on the message's start for a viewer, resolved from the registry for the agent, and its agent joins the job's set; a retry writes nothing twice; one on a run with no message is ignored.
- `tests/ui_catalog.rs`: the UI catalog on a run: a first run records it first and every snapshot names it, a thread without one says nothing (and `null` is none), the same request again attaches and records nothing twice (and ignores a newer catalog), a newer version on a later run is recorded and an older one never becomes current, an answer can carry a newer catalog, the 400 for each rule of the envelope and of the schemas (the reason in `detail`), the 413, and a bad catalog refused on a continuing thread and on an attach with nothing written.
- `tests/roles.rs`: nobody follows or runs another person's thread, an administrator included (404, nothing sent to the agent), a role that names some agents runs only those (and reads capabilities only of those), a person whose roles grant nothing is 403 `no_access` before any stream, and a stream ends with its token and goes on without an expiry.
- `tests/send.rs`: a message while a run is open, over HTTP with the dispatcher running: `steer` and `interrupt` each served (the response is a run of its own and does not echo the message; the first response ends at the message with the invocation suspended; the log says `delivery`; a stop cancels the task and the next job is in the run of the message), no member is a 409 that says what would be served, a malformed one a 400 with nothing written (also for a new thread), only a message is served (two messages, nothing new, a reused run id, an action, someone else's thread), a retried send attaches, and a viewer reconnecting from every resume point gets the rest and nothing else.
- `tests/refusals.rs`: every status of the table in `docs/api/agui.md` (400, 401, 404, 406, 409, 413,
  415, 422, 502) as a problem, with nothing written; another owner's thread id; a message on a finished
  thread is served as the next job (and its retry is an attach, not a third job), a run while another is open is a 409.
- `tests/connect.rs`: a finished thread replayed and left open (only keepalives while idle), runs that
  come later followed, several viewers each getting the whole stream, a reconnect in the middle of a run
  (the preamble, then the rest once), a reconnect from every resume point of a two-run thread, a cursor
  at or beyond the end, `?mode=run` on an idle thread and on a running one, a closed connection not
  cancelling, the stream ending at shutdown, every refusal (404 for missing, malformed and foreign
  threads with one body, 401, 400, 406).
- `tests/registry.rs`: the registry's agents on the AG-UI routes, over a `CompositeRegistry` of the static agents and a `MemoryRegistry` the tests change: an agent is a 404 until the registry lists it, then its capabilities are described and a run streams and finishes, and a 404 again once the registry stops listing it; a registry that is down is a 503 with `Retry-After` and the fixed detail "the agent registry is unreachable" for the registry's agent and for an agent nobody lists (never a 404), the static agents still run, and a registry that is back is read again. `tests/contract.rs` also drives the capabilities 503 against the documented statuses.
- `tests/capabilities.rs`: the document conforms and describes the agent, release channels are declared
  only while the live card lists them, 404 and 401.
- `tests/a2ui.rs`: a surface reaches the requester and a later viewer whole; an action is delivered to the same task and answers the wait; an action for an unknown surface, on a new thread, malformed, oversized (413), beside a message, on someone else's thread, on another agent's thread, on a finished thread, or under a reused run id is refused before the stream with nothing written or sent; the capabilities document declares A2UI only while the live card lists it (each URI, both, card down, card changed).
- `tests/history.rs` (ADR 0059): the pages of a thread tile its connect stream over HTTP for every `limit`, the newest `end` is a settled point a connect resumes from (and what follows it is the next run, in full), the chain still open is not in a page, a catch-up gives the chains after a point and the anchor of the last run, every refusal is a problem (401, the 404 of another person's thread and of an id that is not one, every 400 with its parameter, 406, 403 for roles that hold nothing), a deleted thread is a 404, the reads are counted at `/metrics` without naming a thread or a person, a shared thread is paged over the reader projection (the owner is "the owner", the pages tile the reader's stream, a dead or revoked link is the one 404 with one body), and a public read takes its permit before it reads anything, gives it back on every exit, and is refused while the link's streams are taken.
- `tests/contract.rs`: `docs/api/chat-api.yaml` against this surface. It drives `runAgent`,
  `connectThread`, `connectSharedThread`, `connectPublicSharedThread`, the three history operations and `getAgentCapabilities` and fails when the statuses the contract documents differ
  from the ones answered (one named exemption: a store that fails to read, the 503 of the connect and history operations),
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
