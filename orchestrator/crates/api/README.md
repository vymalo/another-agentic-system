# orch-api

The HTTP edge of the orchestrator: an axum 0.8 router with proxy-identity auth,
RFC 9457 problems, the resource API (agents, thread list and details, export, cancel)
and health, over `orch_app::App`. Interaction surfaces plug into it, and
`health_router` serves health alone.

## Where it sits

The always-mounted part of the HTTP interface between the chat UI and the
orchestrator. It depends on [`orch-app`](../app/README.md),
[`orch-ports`](../ports/README.md) (generic over `Ports`) and
[`orch-core`](../core/README.md); it names no adapter and no surface. The
interaction routes live in surface crates that depend on this one
([`orch-surface-agui`](../surface-agui/README.md)); the
binary ([`orchestrator`](../../bin/orchestrator/README.md)) mounts the ones
`ORCH_SURFACES` names
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)).

## API at a glance

| Item | What |
|---|---|
| `router::<P>(Arc<App<P>>, ApiConfig) -> axum::Router` | health and the resource API, no interaction surface |
| `health_router::<P>(Arc<App<P>>) -> axum::Router` | `/healthz`, `/readyz` and `/metrics` only, no identity, every other path 404: for a process with no HTTP interface (a worker-only orchestrator). The full routers serve the same routes |
| `router_with_surfaces::<P>(app, ApiConfig, Vec<SurfaceRoutes>)` | the same plus the routes of the given surfaces, all behind the identity layer |
| `SurfaceRoutes` | what a surface contributes: `plain(Router)` (request timeout applies), `streaming(Router)` (SSE, no timeout) and `machine(Router, guard)`; already bound to the surface's own state |
| `SurfaceRoutes::machine(routes, guard)` | routes for a caller that is not a person behind oauth2-proxy (an MCP client with a bearer token, later a webhook): **outside** the identity layer and the request timeout, wrapped in `guard`, a tower layer that is the surface's own authentication and a required argument, so a machine route cannot be added without one. It must fail closed and never read `X-Auth-Request-Email` ([ADR 0016](../../../docs/decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md)). The users are [`orch-surface-mcp`](../surface-mcp/README.md) (a bearer token that names a person) and [`orch-surface-thread-tools`](../surface-thread-tools/README.md) (an HMAC token scoped to a thread) |
| `ApiConfig` | `auth`, `sse_keepalive` (15 s; read by surfaces, not by this crate), `request_timeout` (30 s, everything but streaming routes) |
| `AuthConfig { dev_user }`, `IDENTITY_HEADER` | identity handling; `dev_user: None` fails closed |
| `Problem`, `ApiError` | RFC 9457 `application/problem+json` errors, and what a handler can `?` (an `AppError` mapped by its class, or a ready problem) |
| `ApiJson<T>`, `ApiQuery<T>` | extractors whose rejections are 400 problems |
| `EXPORT_FORMAT`, `EXPORT_VERSION` | the `format` (`another-agentic-system/thread-export`) and `version` (1) members of the export document |
| `parse_thread_id` | a path `{threadId}` that is not a UUID is a thread that does not exist |
| `is_host_authority(&str)` | whether a string is a `Host` header value (a name or an address, with or without a port, and nothing else), shared by the surfaces that check `Host`: an allow-list entry that is not one would only look like a rule |
| `sse::keep_alive`, `sse::stream_headers` | the `: keepalive` comment and the no-buffering headers every stream shares |

Routes served here: `GET /healthz`, `GET /readyz`, `GET /metrics`, `GET /api/agents`,
`GET /api/registry` (how each source of agents answered on a read made now: `{sources: [{name, status: ok | unavailable, detail?}]}`, `Cache-Control: no-store`, no agent card read; `detail` only when `unavailable`, in words fit for a person, never a URL or a credential; the web reads it beside `GET /api/agents` to say that the list is incomplete),
`GET /api/threads`, `GET /api/threads/{id}`, `PATCH /api/threads/{id}` (rename: a body of
`{"title"}` and nothing else, in any state of the thread, see `App::rename_thread`; 400 for a title that cannot be
used or another member), `GET /api/threads/{id}/export`,
`POST /api/threads/{id}/cancel`, `POST /api/threads/{id}/fork` (fork a thread from a point: a body of `{"after": seq}` or `{"replace": seq, "text", "messageId"?}`, optional `target` and `id`, see `App::fork_thread` and [ADR 0029](../../../docs/decisions/0029-forking-a-thread-copies-its-log.md); 201 with the new thread and its `Location`, 200 when `id` names a fork of this thread made already, 400 for a body or text or target that cannot be used, 404, 409 with `code: turn_open` while the turn goes on or for an id another thread has or a conversation with 256 branches, 422 for a point that is not in the log or not a person's message), `GET /api/threads/{id}/branches` (`{root, points: [{seq, index, siblings: [{threadId, seq, title}]}]}`: the messages of the thread that have other versions) and `GET /api/threads?branches=include` (without it the threads made by an edit are left out of the list). The interaction routes come from a
surface (`/agui/*`, from `orch-surface-agui`). Bodies are limited to 1 MiB; request ids are set and
propagated. The four legacy interaction operations (`createThread`, `postMessage`, `listEvents`,
`streamEvents`) were removed on 2026-09-30 (ADR 0012): `POST /api/threads` answers 405 (its path
is served for `GET`) and the other three paths 404, with or without a surface mounted.
`GET /api/agents` is read from the agent registry on every request ([ADR 0022](../../../docs/decisions/0022-platform-provisions-agents-system-discovers-them.md)): the static agents and the registry's, each with `source` and `tags`. A registry that cannot say whether an agent exists (`AppError::RegistryUnavailable`) is a 503 with the fixed detail "the agent registry is unreachable" and `Retry-After: 5`, whichever route asked.

```rust
let app: std::sync::Arc<orch_app::App<_>> = /* built by the composition root */;
let cfg = orch_api::ApiConfig::default();
let agui = orch_surface_agui::routes(app.clone(), cfg.sse_keepalive);
let router = orch_api::router_with_surfaces(app, cfg, vec![agui]);
// axum::serve(listener, router).await
```

Identity comes from `X-Auth-Request-Email`; every path except `/healthz`,
`/readyz` and `/metrics` answers 401 without it, surfaces' paths and unknown ones included, and
a present but malformed header is refused even when a dev user is configured.
**The header is only trustworthy behind a proxy (oauth2-proxy) that strips
client-supplied copies.**

### `GET /api/threads/{id}/export`

The thread as one downloadable JSON document, for the owner to send to a developer
([`docs/orchestrator.md`](../../../docs/orchestrator.md#exporting-a-thread), operation `exportThread` of the contract).
Authorised exactly like `GET /api/threads/{id}`: behind the identity layer (401 without it), and `App::export_thread` reads the
thread as the caller, so another owner's thread, an unknown one and an id that is not a UUID are the same 404. The answer is
`200 application/json` with `Content-Disposition: attachment; filename="thread-<id>.json"` and `Cache-Control: no-store`,
pretty-printed: `{format, version: 1, exportedAt, thread, job, binding, events, eventsTruncated}`. `thread` is the contract `Thread`; `job` is the
whole ledger (which `Thread.job` only summarises), with its `number` (which job of the thread this is, [ADR 0020](../../../docs/decisions/0020-a-thread-is-a-conversation.md)); `events` is the log in order from `seq` 1 exactly as stored, and stops at
`thread.lastSeq`, or earlier when a bound of the read cuts it (`eventsTruncated`; the head is kept, with no gap). No credential of the orchestrator is in a log (the bearer token of an agent is held by
`AgentTransport::A2a` only); the file does hold what people and agents wrote, and the owner's e-mail as the actor of their
messages. Built in `src/export.rs`; unit-free (a `Serialize` struct that borrows the `ThreadExport` the application returns and is written straight to the body, with no `serde_json::Value` copy of the log).

### `GET /metrics`

The outbox queue as Prometheus text (`text/plain; version=0.0.4`), written by hand (four
samples; no metrics crate), read from the store on every scrape through
`App::outbox_stats`. A store failure is 503. The values are global (every replica's rows), so
any process can answer.

| Sample | Meaning |
|---|---|
| `orch_outbox_rows{state="due"}` | rows a worker could claim now: `pending` and due, or `inflight` with a lapsed lease |
| `orch_outbox_rows{state="waiting"}` | rows `pending` in retry backoff |
| `orch_outbox_rows{state="leased"}` | rows `inflight` under a live lease: a worker is on them |
| `orch_outbox_oldest_due_age_seconds` | whole seconds since the oldest due row became due, `0` when none |

The pure `render(&OutboxStats, now)` is unit tested against a golden text. How to scale
workers on it: [`docs/orchestrator.md`](../../../docs/orchestrator.md#observability-and-scaling).

## Features and environment

No Cargo features. The crate reads no environment variables; `AUTH_DEV_USER` is
read by the binary and passed in as `AuthConfig`.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`), over real
HTTP. No environment variables.

* `src/problem.rs` unit tests: the status and `Retry-After` for every error class (a cut a thread does not allow is 422, or 409 with `code: turn_open`).
* `tests/contract.rs` also drives `forkThread` and `listBranches`: a fork from here (201, `Location`, `forkedFrom`, the events validated against `Event`), a repeat with the same `id` (200), another agent as `target`, an edit (queued, answered by the dispatcher, hidden from the list and found by the branches), every refused body (400), a point that is not there (422), an id that is taken and a turn that is going on (409, `turn_open`), and someone else's thread (404).
* `src/metrics.rs` unit tests: the exposition text against a golden, an empty outbox, whole
  seconds and the clamp to zero.
* `tests/edge.rs`: health and `/metrics` without identity; `health_router` serving health
  and metrics only (no identity header needed, 404 elsewhere, 503 when not ready or shutting
  down); the resource API without any
  surface; the removed legacy interaction routes are 404 (405 for `POST /api/threads`), mounted
  surface or not; a mounted surface sits
  behind the identity layer (also with a dev user); streaming routes skip the
  request timeout; several surfaces merge.

* `tests/edge.rs` also holds the export tests: one versioned attachment (headers, `thread` equal to `GET /api/threads/{id}`,
  the whole job, the binding, the log), more than one page of events in order with no repeat, the agent's configured bearer token
  nowhere in the file, the owner only (another identity, an unknown id and a non-UUID are the same 404; no or a malformed identity
  is 401; `POST` is 405).
* `tests/registry.rs`: `GET /api/agents` over a `CompositeRegistry` of the static agents and a `MemoryRegistry`: `source` (`static` / `registry`) on every agent, `tags` only when the registry kept some, the registry's agents after the static ones in the registry's order, read live (an agent added shows on the next request, one removed is gone), and none of the registry's agents while it is down; `GET /api/registry` says each source `ok` or `unavailable` with its detail, never cached, and needs an identity. `tests/contract.rs` checks the response against the schema, `source` and `tags` included.
* `tests/contract.rs`: the resource API against [`docs/api/chat-api.yaml`](../../../docs/api/chat-api.yaml).
  Every operation this crate serves (health, the agent list, the thread list with its paging and
  refusals, one thread, its export, cancel) is driven over real HTTP against the in-memory stack with a
  dispatcher, each response is validated against the contract's schemas, and the test fails when the
  contract has an operation it does not drive (the `/agui/*` ones are `orch-surface-agui`'s). The
  events of a real thread and the golden transcripts (`docs/api/examples/*.events.json`) are
  validated against the contract's `Event` schema, and the validator is shown to bite.
* `tests/edge.rs` also holds the identity tests of the resource API: no identity is 401 on every
  path but the probes (a surface's paths, unknown paths and the removed legacy routes included),
  a blank or malformed header is 401, one user never sees another's thread (the same 404 as for a
  thread that does not exist), identity is case- and space-insensitive, the dev user applies only
  when configured, and the probes report readiness and shutdown while the API keeps answering.

## See also

[`orch-app`](../app/README.md), [`orch-e2e`](../e2e/README.md), and the
web client in [`web/`](../../../web/README.md).
