# CRD schemas for kubeconform

The chart renders three custom resources: a CloudNativePG `Cluster`, its `Database` objects and an `ExternalSecret`. The `Deploy` workflow validates
them with kubeconform against the JSON schemas in this directory.

The `Cluster` and the `ExternalSecret` files are byte-for-byte copies of the ones in [vymalo/another-adam-rs](https://github.com/vymalo/another-adam-rs)
`deploy/coder/tests/schemas`, which are copies from
[datreeio/CRDs-catalog](https://github.com/datreeio/CRDs-catalog) at commit
`ad3b08c5045129d7bb1eeffd8e61719b2c8dd1e2` (fetched 2026-09-29, per that directory's README; copied here 2026-10-03). The `Database` file is
fetched from the same catalog commit, so the two CNPG schemas come from one release of the catalog (fetched 2026-10-04; adam-rs's chart has no
`Database` and so no copy of it):

| File | Resource |
|---|---|
| `postgresql.cnpg.io/cluster_v1.json` | `postgresql.cnpg.io/v1` `Cluster` |
| `postgresql.cnpg.io/database_v1.json` | `postgresql.cnpg.io/v1` `Database` (CloudNativePG 1.25 or later) |
| `external-secrets.io/externalsecret_v1.json` | `external-secrets.io/v1` `ExternalSecret` |

They are vendored because a live catalog that answers HTTP 500 once skipped a whole image job in adam-rs's CI. The core
Kubernetes schemas are still downloaded, and the workflow retries that download.

To update, pick a newer catalog commit and fetch the same three paths. If the chart gains a custom resource (a cert-manager
`Certificate`, say; the Ingress asks for its certificate through an annotation, so none is rendered today), add its schema
here, because kubeconform fails on a kind that has no schema.

```sh
C=<catalog commit>
for f in postgresql.cnpg.io/cluster_v1.json postgresql.cnpg.io/database_v1.json external-secrets.io/externalsecret_v1.json; do
  curl -fsSL -o "deploy/chart/tests/schemas/$f" "https://raw.githubusercontent.com/datreeio/CRDs-catalog/$C/$f"
done
```
