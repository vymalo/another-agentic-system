# orch-surface-chat-api

The legacy interaction surface of the chat API: `createThread`, `postMessage`,
`listEvents` and `streamEvents` of
[`docs/api/chat-api.yaml`](../../../docs/api/chat-api.yaml), moved out of
[`orch-api`](../api/README.md) unchanged. **Deprecated** in favour of AG-UI
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)): it is
mounted only while `ORCH_SURFACES` includes `chat-api`, is off by default once
the web runs on AG-UI, and is removed after that.

## Where it sits

An adapter over [`orch-app`](../app/README.md)'s `App`, like every inbound
surface ([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)):
it depends only on `App`, `orch-core`, `orch-ports` (for the `Ports` bound) and
the shared HTTP pieces of `orch-api` (problems, extractors, `SurfaceRoutes`).
It is a Cargo feature of the binary
([`orchestrator`](../../bin/orchestrator/README.md), feature
`surface-chat-api`, on by default). The resource API (agents, thread list, get
thread, cancel) and health stay in `orch-api` and are always served.

## API at a glance

| Item | What |
|---|---|
| `routes::<P>(Arc<App<P>>, sse_keepalive: Duration) -> orch_api::SurfaceRoutes` | the four operations, ready for `orch_api::router_with_surfaces` |

Routes: `POST /api/threads`, `GET /api/threads/{id}/events`,
`POST /api/threads/{id}/messages`, and the SSE `GET /api/threads/{id}/stream`
(`Last-Event-ID` resume, `: keepalive` comments, no request timeout). Mounted by
`orch-api`, they sit behind the identity layer like every route.

## Features and environment

No Cargo features, no environment variables.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`) and the
scripted agent, over real HTTP. The harness in `tests/support` mounts this
surface on `orch_api::router_with_surfaces`, so the tests cover the composed
router: the contract, the resource API and the surface together.

* `tests/conformance.rs`: drives every operation of `docs/api/chat-api.yaml`
  and validates each response body against the spec's schemas (`jsonschema`,
  `serde_norway`).
* `tests/auth.rs`: fail-closed identity and per-user isolation.
* `tests/flows.rs`: end-to-end flows.
* `tests/sse.rs`: replay then live, resume with `Last-Event-ID`, keepalive.

## See also

[`orch-api`](../api/README.md), [`orch-app`](../app/README.md),
[`orch-e2e`](../e2e/README.md) (the same routes against a fake A2A agent, on
both stores).
