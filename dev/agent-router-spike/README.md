# The S9 spike: does an external authorization's header reach the model?

sys#334 S9 puts **Agent Router** (the former Envoy AI Gateway) in front of the model endpoint, and
**Authorino**, through Envoy Gateway's external authorization, adds the person's run-scoped
`Authorization` to the upstream request. One thing was *unverified*: whether the header the external
authorization adds survives Agent Router's **generated routes** and arrives at the model.

This directory answers that question with a kind cluster, and nothing else:

1. **Envoy Gateway v1.10.0** with Agent Router's own values file (vendored here), then **Agent Router
   v1.3.0** (the CRDs chart, then the controller) — every chart pinned by version and OCI digest in
   `UPSTREAM`, and the script refuses a different digest.
2. A **mock model** (`manifests/10-mock.yaml`): an OpenAI-compatible endpoint whose answer's content
   is the JSON of the request headers it received, plus `/__count` so a refused request can be shown
   to reach no model.
3. An **`AIServiceBackend` + `Backend` + `AIGatewayRoute`** (`manifests/20-aigateway.yaml`): a model
   named by `x-ai-eg-model: spike-model`, routed to the mock. This is the route under test.
4. A **stub external authorizer** (`manifests/30-extauth.yaml`) where Authorino will stand: it answers
   `401` without `X-Run-Id` and `200` with `Authorization: Bearer injected-by-extauth` and
   `X-Auth-User: run-<id>` when it is present. A `SecurityPolicy` copies those two response headers to
   the upstream request. The stub is not Authorino — Authorino's `response.success.headers` is
   documented and verified in its docs; the unknown here is the transport through Agent Router's route,
   which is the same Envoy ext_authz response path.

## Run it

```sh
sh dev/agent-router-spike/up.sh     # about 5 minutes the first time (the images are ~1.5 GB)
sh dev/agent-router-spike/spike.sh  # the proof
sh dev/agent-router-spike/down.sh   # deletes the cluster
```

CI runs the same two scripts in `.github/workflows/agent-router-spike.yml`, on every pull request that
touches this directory; it is not a release gate.

### Prerequisite on the host: the inotify budget

Rootless Docker shares the user's inotify budget (`fs.inotify.max_user_instances`, 128 by default)
with every process of that user. `kube-proxy` creates an inotify instance at startup; when the budget
is exhausted it crash-loops with `failed complete: too many open files`, and the Envoy Gateway
`certgen` job then times out reaching the API Service IP (`dial tcp 10.96.0.1:443: i/o timeout`). The
first attempt on 2026-11-21 hit exactly this, with `inotify_init` failing at the first call (errno 24)
inside a fresh container. The fix is the host owner's:

```sh
sudo sysctl -w fs.inotify.max_user_instances=512 fs.inotify.max_user_watches=524288
```

Deleting the cluster frees the instances its own control plane held, which is why a retry can succeed;
a machine with many browsers and editors open may not have room for another cluster until the limit
is raised.

`spike.sh` sends two requests through the gateway:

- with `X-Run-Id: run-1` and a client-sent `Authorization: Bearer client-sent` — it must get `200`, and
  the mock's answer must show `Authorization` containing `injected-by-extauth` and `X-Run-Id: run-1`;
- without `X-Run-Id` — it must get `401` or `403`, and the mock's counter must not move.

## Result

**Run 2026-11-21, in CI (job `spike` of `.github/workflows/agent-router-spike.yml`, head `ed497cfb`): the
header reaches the model, and two behaviours S9 must carry were found on the way.**

- **Yes: the ext-auth `Authorization` reached the mock through the `AIGatewayRoute`.** The request with
  `X-Run-Id: run-1` and a client-sent `Authorization: Bearer client-sent` got `200`; the mock's answer
  carried `X-Run-Id: ["run-1"]`, `X-Auth-User: ["run-run-1"]`, and `spike.sh`'s assertion on the
  injected `Authorization` passed. The runner's log masks the value as `***`; the `grep` for
  `injected-by-extauth` is the evidence.
- **The HTTP external authorization forwards five headers by default** — `Host`, `Method`, `Path`,
  `Content-Length` and `Authorization` — so `X-Run-Id` never reached the authorizer until the
  `SecurityPolicy` named it: `extAuth.headersToExtAuth: ["X-Run-Id"]`, a **sibling of `http`** (the
  strict decoder refuses it inside `http`). **S9 must carry this line.** The gRPC external
  authorization (the epic's choice, and Authorino's) forwards every header and needs no such list.
- **`headersToBackend` overrides a coexisting header**: the mock saw one `Authorization` value, the
  injected one, not the client's beside it — the client-sent header is replaced, not appended, as the
  API documents.
- **The controller generated an `HTTPRoute`** named `spike` in `default` from the `AIGatewayRoute`
  (`up.sh` prints it), so a `SecurityPolicy` can target the generated route; this spike targeted the
  Gateway, which applies to all its routes.
- **What S9 still adds**: TLS on the internal path from a cert-manager CA, Authorino's `AuthConfig`
  over gRPC with the lease lookup, the NetworkPolicy that keeps the route off the edge, and the
  balance's `model_locked` (the injector's per-call check, not this route's).
