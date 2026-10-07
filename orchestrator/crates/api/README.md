# orch-api

The HTTP edge of the orchestrator: an axum 0.8 router whose identity layer asks the `Authenticator` port,
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
| `ApiConfig` | `sse_keepalive` (15 s; read by surfaces, not by this crate), `request_timeout` (30 s, everything but streaming routes), `public_limits`, and `browser_auth: Option<BrowserAuth { issuer, client_id, scope }>`: what `GET /api/public/auth` says (ADR 0054), a 404 when `None` |
| `IDENTITY_HEADER` | the header the identity layer reads beside `Authorization: Bearer`. Identity itself is the `Authenticator` of the application's `Ports` ([ADR 0033](../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)); `ApiConfig.auth` and `AuthConfig` are gone, a development user is `HeaderAuth::with_dev_user` |
| `Problem`, `ApiError` | RFC 9457 `application/problem+json` errors, and what a handler can `?` (an `AppError` mapped by its class, or a ready problem) |
| `ApiJson<T>`, `ApiQuery<T>` | extractors whose rejections are 400 problems |
| `EXPORT_FORMAT`, `EXPORT_VERSION` | the `format` (`another-agentic-system/thread-export`) and `version` (1) members of the export document |
| `parse_thread_id` | a path `{threadId}` that is not a UUID is a thread that does not exist |
| `PublicLimits`, `PublicLimiter`, `ApiConfig.public_limits`, `ApiConfig::check`, `ApiConfigError`, `StreamPermit`, `PublicAccess`, `SurfaceRoutes::public(routes)`, `RedactedSpan`, `redact_path` | sharing ([ADR 0040](../../../docs/decisions/0040-thread-sharing-by-revocable-link.md)): the routes under `/api/shared/{token}` (signed in) and `/api/public/shared/{token}` (no identity, mounted **outside** the identity layer, rate limited), the limiter (per-link and total token buckets in integer milli-tokens, a surcharge for a failure, a ceiling on open streams; `429` with `Retry-After`), and the request span that never holds a token. See *Shared threads* below |
| `is_host_authority(&str)` | whether a string is a `Host` header value (a name or an address, with or without a port, and nothing else), shared by the surfaces that check `Host`: an allow-list entry that is not one would only look like a rule |
| `sse::keep_alive`, `sse::stream_headers` | the `: keepalive` comment and the no-buffering headers every stream shares |

Routes served here: `GET /healthz`, `GET /readyz`, `GET /metrics`, `GET /api/agents`,
`GET /api/registry` (how each source of agents answered on a read made now: `{sources: [{name, status: ok | unavailable, detail?}]}`, `Cache-Control: no-store`, no agent card read; `detail` only when `unavailable`, in words fit for a person, never a URL or a credential; the web reads it beside `GET /api/agents` to say that the list is incomplete),
`GET /api/config` (the public subset of the configuration, exactly `{"ui": {...}}`: the `ui` section of the file with every key at its effective value, nothing else of it; behind the identity layer like every `/api` route, so 401 without an identity; it changes only when the process restarts; `App::public_config`, [`docs/api/config.md`](../../../docs/api/config.md#get-apiconfig)),
`GET /api/threads` (`?limit`, `?before`, `?branches=include`, and, [ADR 0042](../../../docs/decisions/0042-the-thread-list-is-the-owners.md), `?order=recent|rail` (the default is the flat list newest first; `rail` is the person's own order: the pinned first, then the rest by the place they were given, then the archived, each thread followed by the threads nested under it; `limit` counts the threads of the list's own level and `before` is the last of them) and `?archived=exclude|only|include` (the default leaves the archived out); another value is a 400), `PATCH /api/threads/{id}/rail` (`arrangeThread`: a body of `{pinned}`, `{archived}`, `{nested: false}` and `{place: "top" | {before: id} | {after: id}}` and nothing else, at least one, **in no event**: it needs `thread.read` and ownership, not `thread.write`; 200 with the thread, which says `pinned`, `archived` and `nestedUnder` where they hold, also when the row already is what was asked, which writes nothing; 400 for another body, an unknown member or an id that is not one; 404 for another person's thread; 422 `bad_anchor`, 422 `nested_row` and 422 for `nested: true`, see `App::arrange_thread`), `GET /api/threads/{id}`, `PATCH /api/threads/{id}` (a body of `{"title"}` and/or
`{"description"}` and nothing else, at least one, in any state of the thread, see `App::rename_thread` and
`App::describe_thread` ([ADR 0035](../../../docs/decisions/0035-utility-model-tasks.md)): a description is one line of 0 to 500
characters and an empty one clears it, both final; **both members are checked before either is written**, so a 400, for a
title or a description that cannot be used or another member, changes nothing), `DELETE /api/threads/{id}` (`deleteThread`, [ADR 0043](../../../docs/decisions/0043-deleting-a-thread-erases-it.md), `App::delete_thread`: erases the thread, the threads made from it by an edit, their files and their links; 204; 403 `forbidden` without `thread.delete` (not `thread.write`), 404 for a thread that is not the caller's, one that is missing and one deleted already, **409 `thread_active`** while it works (`queued`, `working`, `verifying`, a running ask: stop it, wait for it to end, delete it), 503 `Retry-After` when it kept changing), `GET /api/threads/{id}/export`,
`GET /api/tool-servers` (the servers the deployment offers for attaching, in its order: `id`, `name`, `description?`, `icon?` (a `data:` URI) and `agents?`, **never a URL, a header, a credential, a tool allow-list or a timeout**; `Cache-Control: no-store`; needs `thread.write`, [ADR 0024](../../../docs/decisions/0024-mcp-tools-attached-per-conversation.md)), `PUT /api/threads/{id}/tools` (a body of exactly `{"servers": [ids]}`, the whole set wanted, in any state of the thread: 200 `{servers}` with the set after, the same set again a 200 that writes nothing; 400 for another body, member or an id that is not one; 403 for a thread the caller may read and not change (`read_only`) or without `thread.write`; 404 for one they may not read; 422 for a server the deployment does not offer for the thread's agent, or more than 16; see `App::set_tools`), `POST /api/threads/{id}/cancel`, `POST /api/threads/{id}/fork``POST /api/threads/{id}/cancel`, `POST /api/threads/{id}/fork` (fork a thread from a point: a body of `{"after": seq}`, `{"after": seq, "text", "messageId"?}` (the fork is made with its first message, `queued`, [ADR 0042](../../../docs/decisions/0042-the-thread-list-is-the-owners.md); no mentions or catalog on this route) or `{"replace": seq, "text", "messageId"?}`, optional `target` and `id`, see `App::fork_thread` and [ADR 0029](../../../docs/decisions/0029-forking-a-thread-copies-its-log.md); 201 with the new thread and its `Location`, 200 when `id` names a fork of this thread made already (for `after` with `text`, by this very request), 400 for a body or text or target that cannot be used, 404, 409 with `code: turn_open` while the turn goes on or for an id another thread has or a conversation with 256 branches, 422 for a point that is not in the log or not a person's message), `GET /api/threads/{id}/branches` (`{root, points: [{seq, index, siblings: [{threadId, seq, title}]}]}`: the messages of the thread that have other versions) and `GET /api/threads?branches=include` (without it the threads made by an edit are left out of the list). The interaction routes come from a
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

Identity is what `app.ports().auth()` makes of the request's credentials: the token of `Authorization: Bearer`
(another scheme is not a bearer; a header that is empty, sent twice or not text is a bearer that is refused) and
`X-Auth-Request-Email` (one that is not text is read as empty). Every path except `/healthz`, `/readyz` and
`/metrics` is refused unless it authenticates, surfaces' paths and unknown ones included, and a credential that
is present and bad is refused even when another would have served.

| Outcome | Response |
|---|---|
| authenticated | the `Principal` (the user, and the roles of the credential) is in the request extensions: a handler takes `Extension<Principal>` and hands it to the application, which enforces the roles ([ADR 0033](../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)); there is no bare `UserId` extension any more |
| authenticated, and the roles grant nothing | 403 problem with `code: no_access`, from every route but `GET /api/me`: a valid token with no role the configuration defines and no `auth.defaultRole` |
| `Missing`, `Invalid` | 401 problem; with `WWW-Authenticate: Bearer realm="orchestrator"` when the authenticator reads bearer tokens, and `, error="invalid_token"` when a token was presented and refused (RFC 6750); when it also reads DPoP (`accepts_dpop()`, `auth.dpop`) a second header `DPoP algs="ES256 EdDSA"`. A refused **proof** is `WWW-Authenticate: DPoP error="invalid_dpop_proof", algs="ES256 EdDSA"` alone, and a token that is bad as a DPoP one, not bound to the proof's key, or **bound and sent as a bearer**, `DPoP error="invalid_token", algs="ES256 EdDSA"` (RFC 9449 §7.1) |
| `Unavailable` (the issuer's keys cannot be fetched), `NotConfigured` | 503 problem with `Retry-After: 5`: nobody is let in, and it is not a refusal of the caller |

`/readyz` is 503 (`not ready: cannot authenticate`) while the authenticator's `ready()` fails, so a pod whose issuer's keys were
never fetched is kept out of the service. `/healthz` does not follow it.
**The identity header is only trustworthy behind a proxy (oauth2-proxy) that strips client-supplied copies**; with
`auth.mode: jwt` it is not read at all.

### Roles and permissions

What a person may do is the application's ([`orch-app`](../app/README.md#api-at-a-glance), `authz`): the handlers pass the
`Principal` and map the answer. `AppError::NotFound` is the 404 `no such thread` (a thread the person may not read is the
answer for one that does not exist), `AppError::Forbidden` is a 403 whose `code` is `forbidden` (a permission or an agent
the roles lack); there is no `read_only` code since ADR 0039, because nobody reads a thread that is not theirs.

* **`GET /api/me`** (`getMe`): `{user, email?, name?, roles, permissions: [{permission, scope?}], agents: {read, invoke}}`, `Cache-Control:
  no-store`. It answers a person whose roles grant nothing too, so that a client can say why every other route is a 403. Never a check: the
  orchestrator enforces every request.
* **`GET /api/threads`**: the caller's own threads, newest first, and nobody else's. An `owner` parameter (the administrators' way to list another person's
  or everyone's threads until [ADR 0039](../../../docs/decisions/0039-nobody-reads-another-persons-thread.md)) is a 400, `owner is not supported (ADR 0039)`, with any value.
* **`Thread.owner`**: every serialised thread says its owner (the e-mail), which is always the caller's own.
* **Streams**: `sse::stream_budget(&principal, now)` is how long a stream may stay open for a credential (its `exp` plus 60 s, at most an hour,
  `None` for one that does not run out) and `sse::bounded(stream, budget)` ends a stream when it is spent; the AG-UI surface applies them to its
  connect and run streams, and the client resumes with `Last-Event-ID` and a fresh token.
* Tests: `tests/rbac.rs` drives it all over HTTP from bearer tokens (`MemoryAuth`) and checks the answers against `docs/api/chat-api.yaml`.

### `GET /api/threads/{id}/export`

The thread as one downloadable JSON document, for the owner to send to a developer
([`docs/orchestrator.md`](../../../docs/orchestrator.md#exporting-a-thread), operation `exportThread` of the contract).
Authorised exactly like `GET /api/threads/{id}`: behind the identity layer (401 without it), and `App::export_thread` reads the
thread as the caller (`thread.read`), so a thread the caller may not read, an unknown one and an id that is not a UUID are the same 404. The answer is
`200 application/json` with `Content-Disposition: attachment; filename="thread-<id>.json"` and `Cache-Control: no-store`,
pretty-printed: `{format, version: 1, exportedAt, thread, job, binding, events, eventsTruncated}`. `thread` is the contract `Thread`; `job` is the
whole ledger (which `Thread.job` only summarises), with its `number` (which job of the thread this is, [ADR 0020](../../../docs/decisions/0020-a-thread-is-a-conversation.md)); `events` is the log in order from `seq` 1 exactly as stored, and stops at
`thread.lastSeq`, or earlier when a bound of the read cuts it (`eventsTruncated`; the head is kept, with no gap). No credential of the orchestrator is in a log (the bearer token of an agent is held by
`AgentTransport::A2a` only); the file does hold what people and agents wrote, and the owner's e-mail as the actor of their
messages. Built in `src/export.rs`; unit-free (a `Serialize` struct that borrows the `ThreadExport` the application returns and is written straight to the body, with no `serde_json::Value` copy of the log).

### `GET /api/threads/{threadId}/artifacts/{sha256}`

A file an agent handed over, from the artifact store ([ADR 0032](../../../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md),
operation `getArtifact`). Behind the identity layer; `App::open_artifact` decides who may read (the permission `artifact.read` of ADR 0033 over the thread:
the owner's and nobody else's, an administrator's included; a role without it is a 403), and **every miss is the same 404**:
a thread that is not the caller's, a thread that does not exist, a hash the thread holds no file for (the file of another thread is not reachable by
hash), a hash that is not 64 lowercase hex digits, and a deployment with no artifact store. `src/artifacts.rs` says how the file is sent:

* **streamed** from the store, never held whole (an inline SVG is the one exception, read to be sanitized, up to `MAX_SVG_INLINE_BYTES`, 2 MiB);
  a store that fails midway ends the response in an error so that a prefix is never taken for the file, and nothing of the error is sent;
* **inline** (no `download`, or `download=0`) only for `orch_core::Preview` types (png, jpeg, gif, webp, svg, `text/plain` and `application/json`, the
  last two with `; charset=utf-8`); every other type is an `attachment`, and so is every `?download=1`, with the original bytes;
* an inline **SVG is the sanitized one** (`orch-svg-clean`); one that is too large or cannot be sanitized is sent as an attachment instead;
* always `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox`
  (`ARTIFACT_CONTENT_SECURITY_POLICY`), `Cache-Control: private, max-age=31536000, immutable` (`ARTIFACT_CACHE_CONTROL`) and `ETag: "<sha256>"`;
* `Content-Disposition` carries the file name cleaned (one name, no path, no control or direction character, 255 bytes), as a printable ASCII
  `filename="…"` fallback (`"`, `\`, `%` and `;` replaced) and, when the name is not that, `filename*=UTF-8''…` (RFC 6266, RFC 5987); no usable name is
  `artifact-<8 hex digits>`; a `download` that is not `0`, `1`, `true` or `false` is a 400.

### Shared threads

([ADR 0040](../../../docs/decisions/0040-thread-sharing-by-revocable-link.md), [`chat-api.yaml`](../../../docs/api/chat-api.yaml).)

| Route | Answer |
|---|---|
| `PUT /api/threads/{id}/share` `{ visibility }` | the owner (with `thread.share`) shares: `200` with the link (the token is in the body of this answer and of a rotation, and nowhere else, `Cache-Control: no-store`); refused when `sharing.mode` is `disabled` or the level is above the cap; the same request again changes nothing |
| `POST /api/threads/{id}/share/rotate` | a new link; the old one stops working at once |
| `DELETE /api/threads/{id}/share` | takes the link down; **needs only ownership**, not `thread.share` |
| `GET /api/shared/{token}`, `GET /api/shared/{token}/artifacts/{sha256}` | signed in (`thread.read`, `artifact.read`): the reader's projection (`SharedThread`) and a file the log names |
| `GET /api/public/shared/{token}`, `GET /api/public/shared/{token}/artifacts/{sha256}` | no identity (an `Authorization` header is ignored): the same for a `public` thread; a file only with `sharing.public.files` |

Every link that does not work is the same `404` with the same body (an unknown token, a bad MAC, a private or revoked thread, a lowered
cap, a file that is not the thread's, an `internal` thread on the public route). Every answer about a shared thread is `Cache-Control:
no-store` and `X-Robots-Tag: noindex, nofollow` (a stream adds `no-transform`); a shared file is never cached. A thread's own `GET` and
listing carry `share` for its owner only. The public layer is `guard`: with no limiter (the composition did not build one) every public
request is that 404, so public sharing fails closed; `ApiConfig::check` refuses a composition with the cap `public` and no limiter (the binary always builds one and maps the refusal to exit 78). The request span holds the
path with the token cut, and `tests/span.rs` pins that no log line of a request holds a token.


### DPoP and `GET /api/public/auth` (ADR 0054)

The identity layer reads `Authorization: DPoP <token>` beside `Bearer` and hands the authenticator a `DpopCredentials`: the token, every
`DPoP` header (a header that is not text is `""`), the request's method and its path **as received** (the `OriginalUri` when a router
nested it, else the URI). It reads no proof itself and never reads `DPoP` without the `DPoP` scheme; two `Authorization` headers are one
refused bearer, as before. Whether a deployment takes DPoP, which origins it is called at and what a proof must say is the authenticator's
([`orch-auth-jwt`](../auth-jwt/README.md)). A stream opened by a DPoP request is bounded by the token's `exp` through the same
`Principal::expires_at` (`sse::stream_budget`) as a bearer's.

`GET /api/public/auth` answers `{"issuer", "clientId", "scope"}` (camel-cased, `Cache-Control: no-store`) from `ApiConfig.browser_auth`, and the
one 404 of a route that does not exist when there is none. It is mounted with the public routes: **outside the identity layer** (no credential is
read, even a bad one beside it is ignored), behind the same rate limiter (a 404 of this route is no guess at a link, so it does not charge the
shared bucket the failure surcharge, since a deployment without browser sign-in is asked once by every page), and with no limiter, as for every public
route, it is the 404.

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
| `threads_deleted_total` | counter: threads deleted by this process, the edits of a deleted thread included ([ADR 0043](../../../docs/decisions/0043-deleting-a-thread-erases-it.md)) |
| `late_input_dropped_total{source="dispatcher"\|"inbox"}` | counter: results and rows a worker held for a thread that was deleted meanwhile, dropped and never retried |
| `thread_purges_pending` | gauge: threads deleted whose files are still to be erased, over every replica's rows (`App::purges_pending`; left out when the store cannot say). It is `0` when the inline purge worked; a value that stays up says the artifact store is down or the sweep is not running |

The pure `render(&OutboxStats, now)` is unit tested against a golden text. How to scale
workers on it: [`docs/orchestrator.md`](../../../docs/orchestrator.md#observability-and-scaling).

## Features and environment

No Cargo features. The crate reads no environment variables; `AUTH_DEV_USER` is
read by the binary and passed in as `AuthConfig`.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`), over real
HTTP. No environment variables.

* `tests/only_identity_answers_401.rs`: **only the identity layer answers 401; a handler must never answer 401** (the web resends a request after a 401, a run's POST included, which is safe only because the 401 comes from `require_identity` before any handler runs, and an agent's own 401 is a 502). The test reads the sources of this crate and of `orch-surface-agui` and fails when a file other than `auth.rs` and `problem.rs` names the status; the table test of `src/problem.rs` asserts that no error of a handler maps to it. A machine route (MCP, thread tools, webhooks) has its own guard and is not called by the web.
* `tests/artifacts.rs` (ADR 0032): the owner's PNG inline with every safety header, an attachment for `download=1` (the original SVG, script and all) and a 400 for any other value, an SVG sanitized inline (the length is the cleaned body's), an SVG that is malformed or over 2 MiB an attachment, only the preview types inline and every other type (html, pdf, zip, markdown, bmp, javascript, xml, octet-stream) an attachment, file names (non-ASCII in both forms, a hostile name unable to end the header or add a parameter, no name named by its hash), the 404s (another person's thread, another thread of the same person, an unknown, short, long, upper-case or non-hex hash, a path trick, a thread that is missing or not an id, with the body of a foreign thread equal to a missing one's) and a 401 without identity, no store (`NoArtifacts`), 24 MiB from the directory store whole and in many pieces, the first bytes arriving while the store still holds the rest, and a store that fails midway (the client gets an error, not a prefix, and nothing of the store's message).
* `src/artifacts.rs` unit tests: the `Content-Disposition` forms and hostile names. `tests/contract.rs` also drives `getArtifact` (401, the 404s of a stack with no store, the 400).
* `tests/contract.rs` also drives `listToolServers` (the deployment's list in its order, no URL) and `putThreadTools` (200 with the set sorted, the same set again with one event, a detach and the empty set with `Thread.tools` omitted, every 400 shape, the 404s, the 422s with no URL in the detail and nothing written) against the contract; `tests/rbac.rs` adds the administrator's 404 on another's thread (read, export, branches, files, every act), the 400 of `?owner=` for every role and the 403 of a role without `thread.write`.
* `tests/contract.rs` also drives `deleteThread` (ADR 0043): the 401, another person's thread and an id that is not a UUID are 404 and delete nothing, a thread that works is a 409 with `code: thread_active` and is untouched, after a cancel and the stop it is a 204 with no body and every read of it is a 404, a second delete is a 404, and the list is empty after the last one; `GET /api/me` lists `thread.delete` with no scope. `tests/rbac.rs` pins the permission (a role that only reads erases its own thread, a role without it is 403 `forbidden` for its own thread, another's and a random id alike, another person's thread is a 404 for a user, a reader and an administrator). `tests/sharing.rs` pins that a deleted shared thread's links answer the one 404 of a dead link, byte for byte, at once, that its files are gone, and that `/metrics` counts it and names nobody.
* `tests/contract.rs` also drives `arrangeThread` and the order of `listThreads` (ADR 0042): pin, place on top, before and after, archive and unarchive, the three archived filters in both orders, a fork nested under its parent and ejected, a page of blocks, every 400 and 422 (`bad_anchor`, `nested_row`, `nested: true`), the 404s (another person's thread, the administrator's, nobody's), and that none of it is in the thread's events; `tests/rbac.rs` that a reader may arrange and a role without `thread.read` may not; `tests/sharing.rs` that a pinned and archived thread reads the same through its link, which keeps working.
* `tests/sharing.rs` (ADR 0040): share, widen, rotate and revoke through the routes with the answers above; the link opened signed in and public; every failure of a link the same 404 (byte for byte); the headers of every answer, refusals and the stream included; a public request ignores an `Authorization` header; the limiter (a link's own bucket, the shared one for guesses, `Retry-After`, the stream ceiling); a role without `thread.share` revokes; the cap lowered hides at once. `tests/span.rs` (its own binary: it installs a global subscriber): a request with a token logs no token. `src/limiter.rs` and `src/trace.rs` unit tests: the buckets in milli-tokens (no float), refill, the failure surcharge, `MAX_LINKS` and the redaction of every shape of path.
* `tests/contract.rs` also drives the seven operations of sharing served here against the contract (`shareThread`, `rotateThreadShare`, `unshareThread`, `getSharedThread`, `getSharedArtifact`, `getPublicSharedThread`, `getPublicSharedArtifact`); the two streams are in `orch-surface-agui`.
* `src/problem.rs` unit tests: the status and `Retry-After` for every error class (a cut a thread does not allow is 422, or 409 with `code: turn_open`).
* `tests/contract.rs` also drives `getConfig` (200 with exactly `{"ui": {"showDescriptions": true}}`, 401 without an identity) and `patchThread` with a `description` (the thread, the listing and the log say it, the event is the person's and valid against `Event`, the same again writes nothing, empty clears it, every refusal writes nothing even beside a good title, a title and a description together, someone else's thread is a 404), and `forkThread` and `listBranches`: a fork from here (201, `Location`, `forkedFrom`, the events validated against `Event`), a fork made with its first message (`after` with `text` and `messageId`: 201, queued, the message after `thread_forked`, the events validated against `Event`; the same request again 200 and no second message, another message with that id 409, an empty or too long text and a `messageId` without `text` 400, the turn going on 409 `turn_open`), a repeat with the same `id` (200), another agent as `target`, an edit (queued, answered by the dispatcher, hidden from the list and found by the branches), every refused body (400), a point that is not there (422), an id that is taken and a turn that is going on (409, `turn_open`), and someone else's thread (404).
* `src/metrics.rs` unit tests (the deleting counters and the purge gauge are golden too, and a store that cannot count the purges leaves only the gauge out): the exposition text against a golden, an empty outbox, whole
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
* `tests/browser_auth.rs` (ADR 0054, over a real `JwtAuth` and a local issuer): a DPoP request is authenticated (ES256 and EdDSA, the query not part of `htu`); a replayed proof, a proof for another method, path or origin, two proofs and none are 401 with `DPoP error="invalid_dpop_proof"`; a proof of another key than the token's, a token with no binding, and a bound token as a bearer are 401 with `error="invalid_token"`; a plain bearer is unchanged and the generic challenge names `Bearer` and `DPoP`; without DPoP the scheme is refused and only `Bearer` is advertised, a bound token as a bearer is what it was; the stream budget of a DPoP request equals a bearer's (the token's life and the leeway); the public route is 200 with the three keys and `no-store` with no identity (and a bad credential beside it ignored), a 404 problem equal to any other 404 when unconfigured, 429 behind the limiter, a 404 with no limiter, and its 404s do not spend the shared bucket. `src/auth.rs` unit tests: the DPoP token and proofs are read as the bearer is (two `Authorization` headers, a value that is not text) and the challenges. `tests/contract.rs` also drives `getPublicAuth`.
* `tests/edge.rs` also holds the identity tests of the resource API: no identity is 401 on every
  path but the probes (a surface's paths, unknown paths and the removed legacy routes included),
  a blank or malformed header is 401, one user never sees another's thread (the same 404 as for a
  thread that does not exist), identity is case- and space-insensitive, the dev user applies only
  when configured (it is the `HeaderAuth` of the test's `PortSet`), and the probes report readiness and shutdown while the API keeps answering.

* `src/auth.rs` unit tests: what is a bearer (the scheme in any case, another scheme none, an empty, repeated or non-text header a bearer that is refused) and an identity header that is not text.
* The challenge (`WWW-Authenticate`), the 503 for an unavailable issuer and `/readyz` are pinned end to end by the binary's smoke test
  `in_jwt_mode_only_a_valid_token_is_an_identity_and_readiness_follows_the_keys` (a real process, a local issuer, Postgres).

## See also

[`orch-app`](../app/README.md), [`orch-e2e`](../e2e/README.md), and the
web client in [`web/`](../../../web/README.md).
