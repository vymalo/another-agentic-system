# CRD schemas for kubeconform

The chart renders six custom resources: a CloudNativePG `Cluster`, its `Database` objects, an `ExternalSecret` and, with `backup.enabled`, a
`ScheduledBackup` and the barman-cloud plugin's `ObjectStore`, and, with `ingress.rateLimit.enabled`, a Traefik `Middleware`. The `Deploy` workflow validates them (and `examples/restore-cluster.yaml` and `examples/restore-cutover.yaml`) with
kubeconform against the JSON schemas in this directory.

The `Cluster` and the `ExternalSecret` files are byte-for-byte copies of the ones in [vymalo/another-adam-rs](https://github.com/vymalo/another-adam-rs)
`deploy/coder/tests/schemas`, which are copies from
[datreeio/CRDs-catalog](https://github.com/datreeio/CRDs-catalog) at commit
`ad3b08c5045129d7bb1eeffd8e61719b2c8dd1e2` (fetched 2026-09-29, per that directory's README; copied here 2026-10-03). The `Database` file is
fetched from the same catalog commit, so the two CNPG schemas come from one release of the catalog (fetched 2026-10-04; adam-rs's chart has no
`Database` and so no copy of it). The `ScheduledBackup` and `ObjectStore` files are fetched from that same commit too (2026-10-17, with `curl` from
`raw.githubusercontent.com/datreeio/CRDs-catalog/<commit>/<path>`; the catalog has both at that commit, HTTP 200). That commit's `ObjectStore` is an
older plugin release than the plugin's current one (its `instanceSidecarConfiguration` has `env`, `resources` and `retentionPolicyIntervalSeconds`, not the
later `logLevel` and `additionalContainerArgs`), so the chart uses only fields the schema knows and `-strict` would reject a newer one:

| File | Resource |
|---|---|
| `postgresql.cnpg.io/cluster_v1.json` | `postgresql.cnpg.io/v1` `Cluster` |
| `postgresql.cnpg.io/database_v1.json` | `postgresql.cnpg.io/v1` `Database` (CloudNativePG 1.25 or later) |
| `postgresql.cnpg.io/scheduledbackup_v1.json` | `postgresql.cnpg.io/v1` `ScheduledBackup` (with `backup.enabled`) |
| `barmancloud.cnpg.io/objectstore_v1.json` | `barmancloud.cnpg.io/v1` `ObjectStore` of the barman-cloud plugin (with `backup.enabled`) |
| `external-secrets.io/externalsecret_v1.json` | `external-secrets.io/v1` `ExternalSecret` |
| `traefik.io/middleware_v1alpha1.json` | `traefik.io/v1alpha1` `Middleware` (with `ingress.rateLimit.enabled`: a `rateLimit`) |

They are vendored because a live catalog that answers HTTP 500 once skipped a whole image job in adam-rs's CI. The core
Kubernetes schemas are still downloaded, and the workflow retries that download.

The `Middleware` file is fetched from the same catalog commit (2026-10-28, `curl` from `raw.githubusercontent.com/datreeio/CRDs-catalog/<commit>/traefik.io/middleware_v1alpha1.json`,
HTTP 200). That commit's schema describes Traefik v3.6's `Middleware` (its `rateLimit` has `average`, `burst`, `period`, `sourceCriterion` and `redis`), so the chart uses
only fields it knows and `-strict` would reject a newer one. The Traefik that `tests/ratelimit-e2e` runs is newer (v3.7.16) and accepts the same fields.

To update, pick a newer catalog commit and fetch the same six paths. If the chart gains a custom resource (a cert-manager
`Certificate`, say; the Ingress asks for its certificate through an annotation, so none is rendered today), add its schema
here, because kubeconform fails on a kind that has no schema.

```sh
C=<catalog commit>
for f in postgresql.cnpg.io/cluster_v1.json postgresql.cnpg.io/database_v1.json postgresql.cnpg.io/scheduledbackup_v1.json \
         barmancloud.cnpg.io/objectstore_v1.json external-secrets.io/externalsecret_v1.json traefik.io/middleware_v1alpha1.json; do
  curl -fsSL -o "deploy/chart/tests/schemas/$f" "https://raw.githubusercontent.com/datreeio/CRDs-catalog/$C/$f"
done
```
