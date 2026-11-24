# The S9 scenario: does an external authorization's header reach the model?

sys#334 S9 puts **Agent Router** (the former Envoy AI Gateway) in front of the model endpoint, and
**Authorino**, through Envoy Gateway's external authorization, adds the run's `Authorization` to the
upstream request. The chart's injector (`deploy/chart`, values.yaml `injector`) is that deployment.
This directory is the scenario that proves the two halves the card asks for, with the **real Authorino**
over gRPC:

- **the upstream saw the credential** — the mock model's answer is the JSON of the headers it received,
  and the `Authorization` there is the one the broker's lease handed out;
- **the agent never did** — the agent sends its own `Authorization`, and the model sees exactly one
  header, the leased one: the client's is replaced, not appended.

The scenario runs on a kind cluster, and nothing else:

1. **Envoy Gateway v1.10.0** with Agent Router's own values file (vendored here), then **Agent Router
   v1.3.0** (the CRDs chart, then the controller) — every chart pinned by version and OCI digest in
   `UPSTREAM`, and the script refuses a different digest.
2. **Authorino** through its operator, pinned by the tarball's SHA-256 (`0.25.4`, whose
   `RELATED_IMAGE_AUTHORINO` is `quay.io/kuadrant/authorino:v0.26.4`, the version this scenario
   verified). TLS on the ext-auth hop is off: Envoy Gateway v1.10's `extAuth.grpc` has no TLS field
   (*verified 2026-11-24*); the chart says the same.
3. A **mock model** (`manifests/10-mock.yaml`): an OpenAI-compatible endpoint whose answer's content
   is the JSON of the request headers it received, plus `/__count` so a refused request can be shown
   to reach no model.
4. A **mock of the broker's run-lease route** (`manifests/15-broker.yaml`): `GET
   /internal/runs/<run>/lease` answers `{"token","expiresAt"}` only with the injector's own token, and
   records the run and whether that token was presented. It stands where the orchestrator's internal
   listener stands; the chart points the same `AuthConfig` at the real one.
5. An **`AIServiceBackend` + `Backend` + `AIGatewayRoute`** (`manifests/20-aigateway.yaml`): a model
   named by `x-ai-eg-model: spike-model`, routed to the mock. This is the route under test.
6. **Authorino + `AuthConfig`** (`manifests/30-authorino.yaml`): authenticate (anonymous here, so the
   scenario isolates the transport and the lookup), ask the broker for `X-Run-Id`'s lease with the
   injector's token, cache it per run for 30 s, and add `Authorization` upstream only, as a
   `plain.selector` — **never** `plain.value`, which Authorino v0.26.4 emits as a Go struct dump
   (`{[34 66 …] <nil>}`), not a string (*verified 2026-11-24*, and the chart's comment says so).
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

`spike.sh` makes three checks, two of which can fail:

- with `X-Run-Id: run-1` and a client-sent `Authorization: Bearer client-sent-DO-NOT-PASS` — `200`, the
  model saw exactly **one** `Authorization`, `Bearer anr250-lease-marker-7f3c-run-1` (the token the
  broker minted for `run-1`), and not the client's; the broker saw `run-1` and that the injector's
  token was presented;
- **negative control 1**: with the `SecurityPolicy` deleted, the same request reaches the model with
  the **client's** header. This is what makes the check able to fail; the policy is then re-applied
  and the injected header is seen again;
- **negative control 2**: without `X-Run-Id`, the request is refused (`401`/`403`) and the model's
  counter does not move.

## What this scenario does not prove

The agent's identity (`kubernetesTokenReview`) and the broker's caller rules are the chart's and the
orchestrator's; this scenario authenticates anonymously so that only the transport and the lease
lookup are under test. TLS on the ext-auth hop, TLS on the agent-to-Gateway hop, the NetworkPolicy
that keeps the route off the edge, and the balance's `model_locked` are not here either. The chart
renders the objects; this directory proves the header path they depend on.
