# orch-api

The chat HTTP API: an axum 0.8 router implementing
[`docs/api/chat-api.yaml`](../../../docs/api/chat-api.yaml) over `orch_app::App`.

## Where it sits

The HTTP edge, the only interface between the chat UI and the orchestrator.
It depends on [`orch-app`](../app/README.md), [`orch-ports`](../ports/README.md)
(generic over `Ports`) and [`orch-core`](../core/README.md); it names no
adapter. The binary ([`orchestrator`](../../bin/orchestrator/README.md)) mounts it.

## API at a glance

| Item | What |
|---|---|
| `router::<P>(Arc<App<P>>, ApiConfig) -> axum::Router` | every operation of the contract |
| `ApiConfig` | `auth`, `sse_keepalive` (15 s), `request_timeout` (30 s, everything but SSE) |
| `AuthConfig { dev_user }` | identity handling; `dev_user: None` fails closed |
| `Problem` | RFC 9457 `application/problem+json` errors |

Routes: `GET /healthz`, `GET /readyz`, `GET /api/agents`,
`GET|POST /api/threads`, `GET /api/threads/{id}`,
`GET /api/threads/{id}/events`, `POST /api/threads/{id}/messages`,
`POST /api/threads/{id}/cancel`, `GET /api/threads/{id}/stream` (SSE with
`Last-Event-ID` resume and `: keepalive` comments). Bodies are limited to
1 MiB; request ids are set and propagated.

```rust
let app: std::sync::Arc<orch_app::App<_>> = /* built by the composition root */;
let router = orch_api::router(app, orch_api::ApiConfig::default());
// axum::serve(listener, router).await
```

Identity comes from `X-Auth-Request-Email`; every path except `/healthz` and
`/readyz` answers 401 without it, and a present but malformed header is
refused even when a dev user is configured. **The header is only trustworthy
behind a proxy (oauth2-proxy) that strips client-supplied copies.**

## Features and environment

No Cargo features. The crate reads no environment variables; `AUTH_DEV_USER` is
read by the binary and passed in as `AuthConfig`.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`) and the
scripted agent, over real HTTP. No environment variables.

* `tests/conformance.rs`: drives every operation of `docs/api/chat-api.yaml`
  and validates each response body against the spec's schemas (`jsonschema`,
  `serde_norway`).
* `tests/auth.rs`: fail-closed identity and per-user isolation.
* `tests/flows.rs`: end-to-end flows.
* `tests/sse.rs`: replay then live, resume with `Last-Event-ID`, keepalive.

## See also

[`orch-app`](../app/README.md), [`orch-e2e`](../e2e/README.md), and the
web client in [`web/`](../../../web/README.md).
