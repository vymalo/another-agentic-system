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
| `routes::<P>(Arc<App<P>>, sse_keepalive: Duration) -> orch_api::SurfaceRoutes` | the four operations, each answering with `Deprecation`, ready for `orch_api::router_with_surfaces` |
| `DEPRECATED_AT: i64`, `DEPRECATION: &str` | the deprecation day (unix seconds) and the header value `@1790640000` |

Routes: `POST /api/threads`, `GET /api/threads/{id}/events`,
`POST /api/threads/{id}/messages`, and the SSE `GET /api/threads/{id}/stream`
(`Last-Event-ID` resume, `: keepalive` comments, no request timeout). Mounted by
`orch-api`, they sit behind the identity layer like every route.

## Deprecation

Every response of the four operations, an error included, carries `Deprecation: @1790640000`
([RFC 9745](https://www.rfc-editor.org/rfc/rfc9745): an sf-date, the unix time of 2026-09-29T00:00:00Z),
added by a layer on this surface's own routes: the resource API, health, the AG-UI surface and a 401 from
the shared identity layer never carry it. `DEPRECATED_AT` and `DEPRECATION` are public. No `Sunset` and no
`Link` (see [`docs/api/agui.md`](../../../docs/api/agui.md#the-contract)); the operations are `deprecated:
true` in the contract.

## Features and environment

No Cargo features, no environment variables.

## Tests

Offline: the in-memory stack from `orch-ports` (feature `testkit`) and the
scripted agent, over real HTTP. The harness in `tests/support` mounts this
surface on `orch_api::router_with_surfaces`, so the tests cover the composed
router: the contract, the resource API and the surface together.

* `tests/conformance.rs`: drives every operation of `docs/api/chat-api.yaml`
  (the `/agui/*` ones are `orch-surface-agui`'s) and validates each response body against the spec's
  schemas (`jsonschema`, `serde_norway`), and that `Deprecation` is on every response of an operation
  the contract marks `deprecated` and on no other.
* `tests/deprecation.rs`: the contract deprecates exactly the four operations; the header is an sf-date of
  2026-09-29; it is on their successes and their own errors, and not on the resource API (including
  `GET /api/threads`, which shares a path with `createThread`), health, a 401 or an unknown route.
* `tests/auth.rs`: fail-closed identity and per-user isolation.
* `tests/flows.rs`: end-to-end flows.
* `tests/sse.rs`: replay then live, resume with `Last-Event-ID`, keepalive.

## See also

[`orch-api`](../api/README.md), [`orch-app`](../app/README.md),
[`orch-e2e`](../e2e/README.md) (the same routes against a fake A2A agent, on
both stores).
