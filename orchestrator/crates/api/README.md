# orch-api

The HTTP edge of the orchestrator: an axum 0.8 router with proxy-identity auth,
RFC 9457 problems, the resource API (agents, thread list and details, cancel)
and health, over `orch_app::App`. Interaction surfaces plug into it.

## Where it sits

The always-mounted part of the HTTP interface between the chat UI and the
orchestrator. It depends on [`orch-app`](../app/README.md),
[`orch-ports`](../ports/README.md) (generic over `Ports`) and
[`orch-core`](../core/README.md); it names no adapter and no surface. The
interaction routes live in surface crates that depend on this one
([`orch-surface-chat-api`](../surface-chat-api/README.md), later AG-UI); the
binary ([`orchestrator`](../../bin/orchestrator/README.md)) mounts the ones
`ORCH_SURFACES` names
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)).

## API at a glance

| Item | What |
|---|---|
| `router::<P>(Arc<App<P>>, ApiConfig) -> axum::Router` | health and the resource API, no interaction surface |
| `router_with_surfaces::<P>(app, ApiConfig, Vec<SurfaceRoutes>)` | the same plus the routes of the given surfaces, all behind the identity layer |
| `SurfaceRoutes` | what a surface contributes: `plain(Router)` (request timeout applies) and `streaming(Router)` (SSE, no timeout); already bound to the surface's own state |
| `ApiConfig` | `auth`, `sse_keepalive` (15 s; read by surfaces, not by this crate), `request_timeout` (30 s, everything but streaming routes) |
| `AuthConfig { dev_user }`, `IDENTITY_HEADER` | identity handling; `dev_user: None` fails closed |
| `Problem`, `ApiError` | RFC 9457 `application/problem+json` errors, and what a handler can `?` (an `AppError` mapped by its class, or a ready problem) |
| `ApiJson<T>`, `ApiQuery<T>` | extractors whose rejections are 400 problems |
| `parse_thread_id` | a path `{threadId}` that is not a UUID is a thread that does not exist |
| `sse::keep_alive`, `sse::stream_headers` | the `: keepalive` comment and the no-buffering headers every stream shares |

Routes served here: `GET /healthz`, `GET /readyz`, `GET /api/agents`,
`GET /api/threads`, `GET /api/threads/{id}`,
`POST /api/threads/{id}/cancel`. The interaction operations
(`createThread`, `postMessage`, `listEvents`, `streamEvents`) come from a
surface. Bodies are limited to 1 MiB; request ids are set and propagated.
Without a surface `POST /api/threads` answers 405 (its path is served for `GET`)
and the other interaction paths 404.

```rust
let app: std::sync::Arc<orch_app::App<_>> = /* built by the composition root */;
let cfg = orch_api::ApiConfig::default();
let chat = orch_surface_chat_api::routes(app.clone(), cfg.sse_keepalive);
let router = orch_api::router_with_surfaces(app, cfg, vec![chat]);
// axum::serve(listener, router).await
```

Identity comes from `X-Auth-Request-Email`; every path except `/healthz` and
`/readyz` answers 401 without it, surfaces' paths and unknown ones included, and
a present but malformed header is refused even when a dev user is configured.
**The header is only trustworthy behind a proxy (oauth2-proxy) that strips
client-supplied copies.**

## Features and environment

No Cargo features. The crate reads no environment variables; `AUTH_DEV_USER` is
read by the binary and passed in as `AuthConfig`.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`), over real
HTTP. No environment variables.

* `src/problem.rs` unit tests: the status and `Retry-After` for every error class.
* `tests/edge.rs`: health without identity; the resource API without any
  surface; interaction routes absent unless mounted; a mounted surface sits
  behind the identity layer (also with a dev user); streaming routes skip the
  request timeout; several surfaces merge.

The contract conformance, auth, flow and SSE tests exercise the composed router
(resource API plus the chat-api surface) and live in
[`orch-surface-chat-api`](../surface-chat-api/README.md).

## See also

[`orch-app`](../app/README.md), [`orch-e2e`](../e2e/README.md), and the
web client in [`web/`](../../../web/README.md).
