# orch-surface-webhook

The webhook surfaces: CI systems report the result of a check on a pushed commit, signed, and the report becomes
an inbox row that a worker applies to the job that pushed the commit
([ADR 0017](../../../docs/decisions/0017-ci-results-by-webhook.md), wire contract
[`docs/api/webhooks.md`](../../../docs/api/webhooks.md)).

**Built:** the generic route `POST /webhooks/ci` (`ORCH_SURFACES` name `webhook-generic`, MVP slice 6).
**Planned:** `POST /webhooks/github` (`webhook-github`, slice 9).

## Where it sits

An adapter over [`orch-app`](../app/README.md)'s `App`, like every inbound surface
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)). It depends on `App`,
[`orch-core`](../core/README.md), `orch-ports` (for the `Ports` bound and the clock) and the shared HTTP pieces of
[`orch-api`](../api/README.md) (`Problem`, `SurfaceRoutes`). It decides nothing about a job: it authenticates a
delivery, normalises it to a `CiReport` and calls `App::receive`; the inbox worker and the pure core do the rest.
It is a Cargo feature of the binary ([`orchestrator`](../../bin/orchestrator/README.md), `surface-webhook`, on by
default) and is mounted by `ORCH_SURFACES`.

## API at a glance

| Item | What |
|---|---|
| `generic::routes::<P>(Arc<App<P>>, GenericConfig) -> orch_api::SurfaceRoutes` | `POST /webhooks/ci` as a **machine route**: outside the identity layer and the request timeout, behind its own guard |
| `GenericConfig { secrets, max_skew }`, `GenericConfig::new(secrets)` | the shared secrets and the accepted difference between the timestamp and the clock (default 300 s) |
| `Secrets::parse("a,b")`, `Secrets::new(..)`, `MAX_SECRETS` (2), `SecretsError` | one or two secrets, for rotation; `Debug` shows the count, never a value; an empty list or a third secret is an error whose message carries no secret |
| `signature::sign_generic(secret, timestamp, body)` | the sender's side: `sha256=` and the lowercase hex HMAC-SHA-256 of `"<timestamp>.<body>"`; for tests and tools |
| `generic::PATH`, `MAX_BODY_BYTES` (256 KiB), `MAX_SUMMARY_BYTES` (16 KiB), `SOURCE` (`generic`), the header names | constants of the contract |

### One delivery

The guard runs first and writes nothing; every refusal is an RFC 9457 problem.

1. The three headers `X-Vymalo-Signature-256`, `X-Vymalo-Timestamp` and `X-Vymalo-Delivery` are present: **401**.
2. The timestamp is ASCII digits within the skew of `App`'s clock (inclusive): **401**.
3. The body is at most 256 KiB, declared or not: **413**.
4. The signature is the HMAC-SHA-256 of `"<timestamp>.<body>"` under **either** secret, compared in constant time
   against every secret: **401**.

Then the handler, on the verified bytes only (it refuses if it finds none, so a route cannot be mounted without its
guard): the delivery id is a UUID, the body is `{version: 1, repository, sha, branch?, name, conclusion, url?, summary?}`
with one of the eight conclusions (**400**), and `App::receive("generic", <delivery>, CiReport)` stores the report and
answers **202**, also for a delivery id it has seen (the first delivery wins). A `202` means received, not applied.

## Security notes

- Nothing is written before the signature is good. `orch-e2e` and the crate's tests ask the store for any row after a
  batch of refused deliveries.
- The route never reads `X-Auth-Request-Email`, and a header sent with it changes nothing.
- The timestamp is plain digits, so `"<timestamp>.<body>"` cannot be ambiguous.
- Only `http` and `https` links are kept; the summary is cut to 16 KiB; both are untrusted text downstream.

## Tests

`cargo test -p orch-surface-webhook`, no database:

- `signature.rs` (unit): RFC 4231 test case 2, the known-answer vector of `docs/api/webhooks.md`, that the timestamp and
  every byte of the body are signed, that either of two secrets verifies and a third does not, and that no malformed
  header verifies.
- `secrets.rs` (unit): parsing, at most two, nothing leaks into `Debug` or an error.
- `tests/generic.rs`: over real HTTP, on the in-memory store and a clock the test holds: the report is stored and
  202; a redelivery is 202 and one row; ten kinds of refusal are 401 and the store holds no row; the skew window's
  boundaries and a moving clock; secret rotation; 413 declared and chunked, and exactly at the limit; nineteen malformed
  bodies and three bad delivery ids are 400 and write nothing; optional and unknown members; links and long summaries;
  no identity needed, and the rest of the API still behind the identity layer.

The end-to-end scenarios (a report before its watch, a red report, a deadline, on both stores) are `orch-e2e`'s
`webhook.rs`; the binary's smoke test posts a signed report to a real process.

## See also

[`docs/api/webhooks.md`](../../../docs/api/webhooks.md), [`orch-api`](../api/README.md) (`SurfaceRoutes::machine`),
[`docs/orchestrator.md`](../../../docs/orchestrator.md#the-webhook-surface-mvp-slice-6).
