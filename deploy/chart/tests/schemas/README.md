# CRD schemas for kubeconform

The chart renders two custom resources: a CloudNativePG `Cluster` and an `ExternalSecret`. The `Deploy` workflow validates
them with kubeconform against the JSON schemas in this directory.

Both files are byte-for-byte copies of the ones in [vymalo/another-adam-rs](https://github.com/vymalo/another-adam-rs)
`deploy/coder/tests/schemas`, which are copies from
[datreeio/CRDs-catalog](https://github.com/datreeio/CRDs-catalog) at commit
`ad3b08c5045129d7bb1eeffd8e61719b2c8dd1e2` (fetched 2026-09-29, per that directory's README; copied here 2026-10-03):

| File | Resource |
|---|---|
| `postgresql.cnpg.io/cluster_v1.json` | `postgresql.cnpg.io/v1` `Cluster` |
| `external-secrets.io/externalsecret_v1.json` | `external-secrets.io/v1` `ExternalSecret` |

They are vendored because a live catalog that answers HTTP 500 once skipped a whole image job in adam-rs's CI. The core
Kubernetes schemas are still downloaded, and the workflow retries that download.

To update, pick a newer catalog commit and fetch the same two paths. If the chart gains a custom resource (a cert-manager
`Certificate`, say; the Ingress asks for its certificate through an annotation, so none is rendered today), add its schema
here, because kubeconform fails on a kind that has no schema.

```sh
C=<catalog commit>
for f in postgresql.cnpg.io/cluster_v1.json external-secrets.io/externalsecret_v1.json; do
  curl -fsSL -o "deploy/chart/tests/schemas/$f" "https://raw.githubusercontent.com/datreeio/CRDs-catalog/$C/$f"
done
```
