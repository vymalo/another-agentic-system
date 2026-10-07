#!/usr/bin/env sh
# The checks of dev/kagent/ that need no cluster (the Kagent E2E workflow runs them first, on every pull request that touches the folder):
#
#   dev/kagent/check.sh
#
#   * the charts pulled by version have the digests of dev/kagent/UPSTREAM (four for kagent 1.x and Substrate, two for kagent 0.10);
#   * the kagent chart renders with dev/kagent/kagent-values.yaml, runs the controller and the bundled Postgres at the pinned digests and the
#     WorkerPool's worker at its pinned digest, and has no UI replica, no tool server and no kmcp (what the values leave out stays out);
#   * every digest that kagent-values.yaml, manifests.yaml and kind-config.yaml name is one UPSTREAM records (they are copies, and drift);
#   * dev/kagent/manifests.yaml's kagent resources are valid against the CRDs of the pinned CRDs chart (their schemas, not their CEL rules);
#   * the same for kagent 0.10 (second section of UPSTREAM): its chart renders with kagent-values-0.10.yaml, runs the controller and the Go runtime at
#     the pinned digests, has no Substrate, no UI replica, no tool server, no kmcp and no built-in agent, and manifests-0.10.yaml is valid against its CRDs;
#   * the model mock's mappings parse and every plain script has its SSE twin (the runtime streams).
#
# Needs helm, jq, curl, python3 with PyYAML and jsonschema (CI installs them with pip). Exit status 0 when every check passed.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
kdir=$root/dev/kagent
upstream=$kdir/UPSTREAM
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# shellcheck source=dev/kagent/lib.sh
. "$kdir/lib.sh"

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

for tool in helm jq python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "check.sh: $tool is required and was not found" >&2; exit 2; }
done

kagent_version=$(pin kagent_version)
substrate_version=$(pin substrate_version)
kagent_crds=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent-crds "$kagent_version" kagent_crds_digest)
kagent=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent "$kagent_version" kagent_digest)
pull_chart ghcr.io/kagent-dev/substrate/helm/substrate-crds "$substrate_version" substrate_crds_digest >/dev/null
substrate=$(pull_chart ghcr.io/kagent-dev/substrate/helm/substrate "$substrate_version" substrate_digest)
ok "the four charts have the pinned digests"

# --- the render ----------------------------------------------------------------------------------------
helm template kagent "$kagent" --namespace kagent --values "$kdir/kagent-values.yaml" > "$tmp/kagent.yaml"
helm template substrate "$substrate" --namespace ate-system \
  --set-string 'ateApi.extraArgs[0]=--template-resync-interval=250ms' \
  --set 'credentialProvider.namespacePolicies[0].atespace=kagent' \
  --set 'credentialProvider.namespacePolicies[0].allowedNamespaces[0]=kagent' > "$tmp/substrate.yaml"
ok "the charts render with our values"

want_image() { # want_image DESCRIPTION FILE FRAGMENT
  if grep -Eq "image: .*$3" "$2"; then ok "$1"; else bad "$1: no 'image: ...$3' in the render"; fi
}
want_image "the controller runs at the pinned digest" "$tmp/kagent.yaml" "controller:$kagent_version@$(pin kagent_digest | cut -d: -f1)"
controller_digest=$(sed -n 's/^#   ghcr.io\/kagent-dev\/kagent\/controller:'"$kagent_version"' *index \(sha256:[0-9a-f]*\).*/\1/p' "$upstream")
if grep -q "controller:$kagent_version@$controller_digest" "$tmp/kagent.yaml"; then ok "... which is the digest UPSTREAM records for it"; else bad "the controller's digest in kagent-values.yaml is not the one in UPSTREAM ($controller_digest)"; fi
postgres_digest=$(sed -n 's/^#   docker.io\/library\/postgres:[^ ]* *index \(sha256:[0-9a-f]*\).*/\1/p' "$upstream" | head -n 1)
if grep -q "postgres:.*@$postgres_digest" "$tmp/kagent.yaml"; then ok "the bundled Postgres runs at the digest UPSTREAM records"; else bad "the bundled Postgres' digest is not the one in UPSTREAM ($postgres_digest)"; fi
worker_digest=$(sed -n 's/^#   ghcr.io\/kagent-dev\/substrate\/ateom-gvisor:[^ ]* *index \(sha256:[0-9a-f]*\).*/\1/p' "$upstream")
if grep -q "workerImage: .*@$worker_digest" "$tmp/kagent.yaml"; then ok "the WorkerPool's worker runs at the digest UPSTREAM records"; else bad "the worker image's digest is not the one in UPSTREAM ($worker_digest)"; fi
for left_out in kagent-tools grafana kmcp; do
  if grep -q "image: .*$left_out" "$tmp/kagent.yaml"; then bad "the render has an image of $left_out, which the values leave out"; else ok "nothing of $left_out in the render"; fi
done
if grep -q "kagent-ui" "$tmp/kagent.yaml" && ! grep -q "replicas: 0" "$tmp/kagent.yaml"; then bad "the UI Deployment has replicas"; else ok "the UI has no replicas"; fi

# --- the digests the files copy -----------------------------------------------------------------------------
for f in kagent-values.yaml manifests.yaml kind-config.yaml; do
  grep -o 'sha256:[0-9a-f]\{64\}' "$kdir/$f" | sort -u > "$tmp/digests"
  while read -r d; do
    if grep -q "$d" "$upstream"; then ok "$f: $d is in UPSTREAM"; else bad "$f: $d is not in UPSTREAM"; fi
  done < "$tmp/digests"
done

# --- the manifests against the CRDs --------------------------------------------------------------------------
helm template crds "$kagent_crds" --set kmcp.enabled=false > "$tmp/crds.yaml"
validate_manifests() { # validate_manifests CRDS_FILE MANIFESTS_FILE API_GROUP_PREFIX
python3 - "$1" "$2" "$3" <<'EOF' || fail=1
import sys
import yaml
import jsonschema

crds = {}
for doc in yaml.safe_load_all(open(sys.argv[1])):
    if doc and doc.get("kind") == "CustomResourceDefinition":
        crds[doc["spec"]["names"]["kind"]] = doc
bad = 0
for doc in yaml.safe_load_all(open(sys.argv[2])):
    if not doc or not doc["apiVersion"].startswith(sys.argv[3]):
        continue
    version = doc["apiVersion"].split("/")[1]
    served = [v for v in crds[doc["kind"]]["spec"]["versions"] if v["name"] == version]
    if not served:
        print(f"FAIL {doc['kind']} {doc['metadata']['name']}: the CRD has no version {version}")
        bad = 1
        continue
    # The CRDs write regular expressions with POSIX classes, which Python's `re` reads as something else: those messages are not ours.
    errors = [e for e in jsonschema.Draft7Validator(served[0]["schema"]["openAPIV3Schema"]).iter_errors(doc)
              if "[:" not in str(e.schema.get("pattern", ""))]
    if errors:
        print(f"FAIL {doc['kind']} {doc['metadata']['name']}: " + "; ".join(e.message[:160] for e in errors))
        bad = 1
    else:
        print(f"ok   {doc['kind']} {doc['metadata']['name']} is valid against the CRD")
sys.exit(bad)
EOF
}
validate_manifests "$tmp/crds.yaml" "$kdir/manifests.yaml" "api.kagent.dev/"

# --- the mock model's mappings -----------------------------------------------------------------------------------
plain=$kdir/wiremock/mappings/kagent-mock.json
stream=$kdir/wiremock/mappings/kagent-mock-stream.json
if jq -e '.mappings | length > 0' "$plain" "$stream" >/dev/null; then ok "the mappings parse"; else bad "a mapping file does not parse"; fi
missing=$(jq -n --slurpfile p "$plain" --slurpfile s "$stream" \
  '[$p[0].mappings[].name] - [$s[0].mappings[].metadata.twinOf] | join("; ")')
if [ "$missing" = '""' ]; then ok "every script has its SSE twin"; else bad "scripts without a twin: $missing"; fi

# --- kagent 0.10 (the second section of UPSTREAM) ------------------------------------------------------------------
v010=$(pin kagent010_version)
crds010=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent-crds "$v010" kagent010_crds_digest)
chart010=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent "$v010" kagent010_digest)
ok "the two charts of kagent 0.10 have the pinned digests"
helm template kagent "$chart010" --namespace kagent --values "$kdir/kagent-values-0.10.yaml" > "$tmp/kagent010.yaml"
ok "the 0.10 chart renders with our values"
digest010() { sed -n 's/^#   ghcr.io\/kagent-dev\/kagent\/'"$1"':'"$v010"' *index \(sha256:[0-9a-f]*\).*/\1/p' "$upstream" | head -n 1; }
# the controller's image is in the Deployment; the runtime's is in the controller's ConfigMap (GO_IMAGE_TAG), which the controller turns into the agent's image
d=$(digest010 controller)
if [ -n "$d" ] && grep -Eq "image: .*kagent/controller:$v010@$d" "$tmp/kagent010.yaml"; then ok "0.10: the controller runs at the digest UPSTREAM records ($d)"; else bad "0.10: the controller is not at the digest UPSTREAM records (${d:-none}) in the render"; fi
d=$(digest010 golang-adk)
if [ -n "$d" ] && grep -q "GO_IMAGE_TAG: \"$v010@$d\"" "$tmp/kagent010.yaml"; then ok "0.10: the Go runtime is given at the digest UPSTREAM records ($d)"; else bad "0.10: GO_IMAGE_TAG is not $v010@${d:-none} in the render"; fi
for left_out in kagent-tools grafana kmcp; do
  if grep -q "image: .*$left_out" "$tmp/kagent010.yaml"; then bad "0.10: the render has an image of $left_out, which the values leave out"; else ok "0.10: nothing of $left_out in the render"; fi
done
# 0.10 runs an agent as a Deployment: nothing of Agent Substrate (its namespace, its WorkerPool, its ActorTemplates) is installed or enabled
if grep -Eq 'ate-system|^kind: (WorkerPool|ActorTemplate)$' "$tmp/kagent010.yaml"; then bad "0.10: the render has something of Agent Substrate, which 0.10 does not need"; else ok "0.10: nothing of Agent Substrate in the render"; fi
if grep -Eq '^kind: Agent$' "$tmp/kagent010.yaml"; then bad "0.10: the render has an Agent (a built-in agent the values should turn off)"; else ok "0.10: no built-in agent in the render"; fi
if grep -q "kagent-ui" "$tmp/kagent010.yaml" && ! grep -q "replicas: 0" "$tmp/kagent010.yaml"; then bad "0.10: the UI Deployment has replicas"; else ok "0.10: the UI has no replicas"; fi
if grep -q 'A2A_BASE_URL: "http://kagent010-control-plane:30083"' "$tmp/kagent010.yaml"; then ok "0.10: the card advertises the node's address on the docker network kind"; else bad "0.10: A2A_BASE_URL is not http://kagent010-control-plane:30083 in the render"; fi
for f in kagent-values-0.10.yaml manifests-0.10.yaml kind-config-0.10.yaml; do
  grep -o 'sha256:[0-9a-f]\{64\}' "$kdir/$f" | sort -u > "$tmp/digests"
  while read -r d; do
    if grep -q "$d" "$upstream"; then ok "$f: $d is in UPSTREAM"; else bad "$f: $d is not in UPSTREAM"; fi
  done < "$tmp/digests"
done
helm template crds "$crds010" > "$tmp/crds010.yaml"
validate_manifests "$tmp/crds010.yaml" "$kdir/manifests-0.10.yaml" "kagent.dev/"

if [ "$fail" -eq 0 ]; then echo "kagent static checks passed"; else echo "kagent static checks FAILED"; exit 1; fi
