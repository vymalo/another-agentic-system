---
sidebarTitle: "0103 · Helm 4 only, Kubernetes 1.37 floor"
---

# ADR 0103 — Helm 4 only, and Kubernetes 1.37 as the floor

- **Status:** **Accepted (2026-11-27)** by the owner: *"We'll support helm 4 only from now on. Hard cutover, move everything from helm 3. Support starts at k8s 1.37 too."* The platform's twin is AD-067.

## Context

This repository installs its chart with Helm and proves it on kind clusters. Before this decision it pinned Helm 3
(`v3.18.0` in the kagent and router-spike jobs, `v3.19.0` in `deploy.yml`, the backup, pooler and rate-limit jobs), a chart
floor of `kubeVersion: ">=1.27.0-0"`, `kindest/node:v1.35.0` in every kind job, and kubeconform at Kubernetes 1.31. The
owner moved the estate's floor in one step: Helm 4 only, Kubernetes 1.37 or later, no dual testing and no Helm 3 path.

## Decision

1. **Helm v4.3.2 everywhere**, and nothing else: the six workflows that install it (`deploy.yml`, `backup-e2e.yml`,
   `pooler-e2e.yml`, `ratelimit-e2e.yml`, `kagent-e2e.yml` in its three jobs, `agent-router-spike.yml`), the
   `azure/setup-helm` steps, the manual downloads in the kind jobs, and the README that says which Helm the chart is
   checked with. A document that says "Helm 3" or names a 3.x version is wrong from this date, except a dated record of
   what was run then.
2. **The chart's floor is `kubeVersion: ">=1.37.0-0"`.** Kubernetes below 1.37 is unsupported, not merely untested:
   `helm template --kube-version 1.36.0` fails.
3. **Every kind job runs `kindest/node:v1.37.0`**, by tag and digest
   (`sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5`), with kind v0.33.0 and kubectl v1.37.3
   (the minor of the node, so the client is never two minors from the server) — except the agent-router spike, which keeps
   kubectl v1.35.9, the version it last passed with: `kubectl port-forward` in 1.36 and 1.37 enters the pod's network
   namespace and dials `127.0.0.1` inside it, and Envoy Gateway's proxy binds the pod's address, not loopback, so every
   tunnel to it is refused (runs 45946481869 and 45948777930). The spike is the only job here that port-forwards; it is
   re-pinned when the proxy listens on loopback or the spike reaches the gateway another way.
4. **kubeconform checks the render at 1.37.0** (`KUBERNETES_VERSION` of `deploy.yml`), whose schemas exist
   (`master-1.37.0` of `yannh/kubernetes-json-schema`).
5. **Helm 4's behaviour was read, not assumed**, on the pinned binary:
   - `--wait` takes a strategy now and, alone, is the **kstatus watcher** (the default when the flag is omitted is
     `hookOnly`); every `helm install ... --wait` in the e2e scripts therefore waits on readiness, not only on hooks;
   - `--server-side` defaults to **true** on install and upgrade: objects are applied through the API server, so a
     resource that already exists and carries another manager's field ownership can refuse the install
     (`--take-ownership` and `--force-conflicts` are the escape hatches, and neither is used here);
   - `--force` is `--force-replace`, and `--dry-run` requires `client` or `server`. No script here used either.
6. **CloudNativePG is the one part that cannot meet the floor yet, and it is recorded rather than hidden.** The newest
   chart release, `cloudnative-pg` 0.29.2, ships operator 1.30.2, whose own notes support Kubernetes 1.34 to 1.36; the
   operator release that names 1.37 is 1.31.0-rc1 (2026-11-25) and it has no chart release. The backup and pooler jobs
   therefore run the 1.30.2 operator on a 1.37 node, which its declared support does not cover: *unverified* whether it
   behaves, and to be re-pinned to the 1.31 chart when that is published.

## Consequences

- A deployment on a cluster older than 1.37, or with Helm 3, is unsupported: the chart refuses to render and CI never
  runs it.
- The kind jobs' `helm install --wait` now waits with kstatus, and their installs are server-side applies. Both are
  visible in the e2e runs that this ADR's pull request starts; a hook-only wait that used to pass can now fail on a
  resource that never becomes Ready, which is the behaviour the owner asked for.
- The pinned checksums move with the versions: helm v4.3.2 `sha256:d1c0278d…`, kubectl v1.37.3 `sha256:9a1b55af…`,
  node image `sha256:a1ed56cf…` (get.helm.sh, dl.k8s.io and Docker Hub, *verified 2026-11-27*).
- *Verified 2026-11-27:* the three checksums above; `helm v4.3.2` runs here and its `install`/`template` help carries
  `--wait` (with strategies), `--server-side`, `--force-replace` and `-s/--show-only`; `master-1.37.0` schemas answer
  200; `kindest/node:v1.37.0` answers 200 with the digest above.
- *Unverified:* the e2e jobs on 1.37 with Helm 4 are this ADR's pull request, not a local run; and CloudNativePG
  1.30.2's behaviour on 1.37 (see decision 6).
