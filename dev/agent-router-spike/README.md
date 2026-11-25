# The S9 scenario: does an external authorization's header reach the model?

sys#334 S9 puts **Agent Router** (the former Envoy AI Gateway) in front of the model endpoint, and
**Authorino**, through Envoy Gateway's external authorization, adds the run's `Authorization` and
`x-account-id` to the upstream request. The chart's injector (`deploy/chart`, values.yaml `injector`) is that
deployment. This directory is the scenario that proves the two halves the card asks for, with the **real
Authorino** over gRPC and a **WireMock gateway**:

- **the gateway saw the credential and the account** — WireMock's request journal shows exactly one
  `Authorization`, the one the broker's lease handed out, and `x-account-id` naming the account that lease
  carried;
- **the agent never did** — the agent sends its own `Authorization`, and the gateway sees exactly one
  header, the leased one: the client's is replaced, not appended.

## Result (ANR-662, 2026-11-24): it works; the injector stays Agent Router + Authorino

Through Agent Router v1.3.0 on Envoy Gateway v1.10.0 with Authorino v0.26.4, an ext-auth `ok_response`
header reaches the WireMock gateway: the leased provider key as `Authorization` and the lease's
`accountId` as `x-account-id`. No service of our own is needed behind the port; the injector is the
chart's objects. WireMock is practical in kind (a Deployment with a mapping ConfigMap, the same image the
repo uses for its model stand-in), so the gateway here is WireMock, not an echo pod.

The scenario also found one fail-open, now closed in the chart: when the broker refuses a lease (410
`lease_expired` / 404 `run_not_bound`), Authorino has no metadata but still applies every
`ok_response` header, so the gateway received `Authorization: Bearer` with an empty token. The chart's
`AuthConfig` now denies a lease with no token (`authorization.lease`, selector
`auth.metadata.lease.token`, `neq ""`), and the request is refused at the authorizer. A run with no lease
reaches no gateway at all.

The scenario runs on a kind cluster, and nothing else:

1. **Envoy Gateway v1.10.0** with Agent Router's own values file (vendored here), then **Agent Router
   v1.3.0** (the CRDs chart, then the controller) — every chart pinned by version and OCI digest in
   `UPSTREAM`, and the script refuses a different digest.
2. **Authorino** through its operator, pinned by the tarball's SHA-256 (`0.25.4`, whose
   `RELATED_IMAGE_AUTHORINO` is `quay.io/kuadrant/authorino:v0.26.4`, the version this scenario
   verified). TLS on the ext-auth hop is off: Envoy Gateway v1.10's `extAuth.grpc` has no TLS field
   (*verified 2026-11-24*); the chart says the same.
3. A **WireMock model gateway** (`manifests/10-gateway.yaml`): an OpenAI-compatible endpoint pinned by tag
   and digest in `UPSTREAM`. Its admin API keeps the request journal (`/__admin/requests`), which is the
   observable: every request that reached the gateway, with the headers it arrived with.
4. A **mock of the broker's run-lease route** (`manifests/15-broker.yaml`): `GET
   /internal/runs/<run>/lease` answers `{"token","accountId","expiresAt"}` only with the injector's own
   token, and records the run and whether that token was presented. `expired-run` answers `410
   lease_expired`, `absent-run` answers `404 run_not_bound`. It stands where the orchestrator's internal
   listener stands; the chart points the same `AuthConfig` at the real one.
5. An **`AIServiceBackend` + `Backend` + `AIGatewayRoute`** (`manifests/20-aigateway.yaml`): a model
   named by `x-ai-eg-model: spike-model`, routed to the gateway. This is the route under test.
6. **Authorino + `AuthConfig`** (`manifests/30-authorino.yaml`): authenticate (anonymous here, so the
   scenario isolates the transport and the lookup), ask the broker for `X-Run-Id`'s lease with the
   injector's token, cache it per run for 30 s, and add `Authorization` and `x-account-id` upstream only,
   as `plain.selector`s — **never** `plain.value`, which Authorino v0.26.4 emits as a Go struct dump
   (`{[34 66 …] <nil>}`), not a string (*verified 2026-11-24*, and the chart's comment says so). A
   lease with no token is denied (`authorization.lease`).
7. A **`SecurityPolicy`** (`manifests/31-route-policy.yaml`) with `extAuth.grpc`, scoped to the
   **`HTTPRoute` Agent Router generates** from the `AIGatewayRoute` (same name) rather than the whole
   Gateway — also *verified 2026-11-24*: `targetRefs: kind=HTTPRoute name=spike` reports `Accepted`.

## Run it

```sh
sh dev/agent-router-spike/up.sh     # about 5 minutes the first time (the images are ~1.5 GB)
sh dev/agent-router-spike/spike.sh  # the proof
sh dev/agent-router-spike/down.sh   # deletes the cluster
```

CI runs the same scripts in `.github/workflows/agent-router-spike.yml`, on every pull request that
touches this directory; it is not a release gate.

### Prerequisite on the host: the inotify budget

Rootless Docker shares the user's inotify budget (`fs.inotify.max_user_instances`, 128 by default)
with every process of that user. `kube-proxy` creates an inotify instance at startup; when the budget
is exhausted it crash-loops with `failed complete: too many open files`, and the Envoy Gateway
`certgen` job then times out reaching the API Service IP (`dial tcp 10.96.0.1:443: i/o timeout`).
The first attempt on 2026-11-21 hit exactly this, with `inotify_init` failing at the first call (errno
24) inside a fresh container. The fix is the host owner's:

```sh
sudo sysctl -w fs.inotify.max_user_instances=512 fs.inotify.max_user_watches=524288
```

Deleting the cluster frees the instances its own control plane held, which is why a retry can
succeed; a machine with many browsers and editors open may not have room for another cluster until
the limit is raised.

`spike.sh` makes these checks, all of which can fail:

- with `X-Run-Id: run-1` and a client-sent `Authorization: Bearer client-sent-DO-NOT-PASS` — `200`, the
  gateway journal shows exactly **one** `Authorization`, `Bearer anr250-lease-marker-7f3c-run-1` (the
  token the broker minted for `run-1`), not the client's, and `x-account-id: acct-run-1` (the account id
  in that same lease); the broker saw `run-1` and that the injector's token was presented;
- **negative control 1**: with the `SecurityPolicy` deleted, the same request reaches the gateway with
  the **client's** header. This is what makes the check able to fail; the policy is then re-applied
  and the injected header is seen again;
- **negative control 2**: without `X-Run-Id`, the request is refused (`403`) and the gateway journals no
  new request;
- **negative control 3a**: `X-Run-Id: expired-run`, which the broker answers `410 lease_expired` — the
  authorizer refuses it (`403`) and the gateway journal does not move. A lease that cannot be resolved
  never becomes an empty `Authorization`;
- **negative control 3b**: `X-Run-Id: absent-run`, which the broker answers `404 run_not_bound` — same
  refusal and the same unmoved journal.

## What this scenario does not prove

The agent's identity (`kubernetesTokenReview`) and the broker's caller rules are the chart's and the
orchestrator's; this scenario authenticates anonymously so that only the transport and the lease
lookup are under test. TLS on the ext-auth hop, TLS on the agent-to-Gateway hop, the NetworkPolicy
that keeps the route off the edge, and the balance's `model_locked` are not here either. The chart
renders the objects; this directory proves the header path they depend on.
