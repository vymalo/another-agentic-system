#!/usr/bin/env sh
# Brings up what dev/kagent-e2e.sh needs with KAGENT_VERSION=0.10 (dev/README.md, "kagent"): a kind cluster, kagent 0.10.3 and one agent,
# `kagent/hello`, on a scripted OpenAI-compatible model. Safe to run again: every step is an upgrade or an apply.
#
#   dev/kagent/up-0.10.sh          # about 3 to 5 minutes the first time (the images are a few hundred MB compressed)
#   KAGENT_VERSION=0.10 dev/kagent/down.sh
#
# The light one of the two (dev/kagent/up.sh is kagent 1.x): kagent 0.10 runs a declarative agent as an ordinary Deployment, so there is no Agent
# Substrate, no feature gate, no gVisor and no CA pool: a plain kind node, the two charts, the model and the agent. Every pin is in
# dev/kagent/UPSTREAM (its second section), and a chart is pulled by version and its digest compared with the pinned one first.
#
#   1. a kind cluster `kagent010` (dev/kagent/kind-config-0.10.yaml: the two NodePorts on the host's loopback);
#   2. kagent: the CRDs chart and the chart, with dev/kagent/kagent-values-0.10.yaml (no UI, no tool server, no kmcp, none of the built-in agents);
#   3. dev/kagent/manifests-0.10.yaml: the scripted model (WireMock, with dev/kagent/wiremock/mappings), a ModelConfig and an Agent; and it
#      waits for the agent to be Ready and its card to be served.
#
# Needs on the PATH: docker, kind, kubectl, helm, jq, curl (CI installs kind, kubectl and helm at pinned versions with their checksums).
# It leaves a cluster running and docker pulls behind: `KAGENT_VERSION=0.10 dev/kagent/down.sh` removes the cluster.
#
# Environment: KAGENT_DIAG=0 turns the diagnostics (what failed to become ready, the controller's log) off.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
kdir=$root/dev/kagent
# shellcheck disable=SC2034 # read by pin() of lib.sh
upstream=$kdir/UPSTREAM
cluster=kagent010
ctx=kind-$cluster

kc() { kubectl --context "$ctx" "$@"; }
hc() { helm --kube-context "$ctx" "$@"; }

for tool in docker kind kubectl helm jq curl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "up-0.10.sh: $tool is required and was not found on the PATH" >&2; exit 2; }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# shellcheck source=dev/kagent/lib.sh
. "$kdir/lib.sh"

diagnose() { # diagnose WHAT: what is not ready, to help whoever reads the log
  echo "up-0.10.sh: $1" >&2
  [ "${KAGENT_DIAG:-1}" = 0 ] && return 0
  {
    echo "--- pods"
    kc get pods -A -o wide
    echo "--- the agent and its model"
    kc -n kagent get agents.kagent.dev,modelconfigs.kagent.dev -o yaml | head -n 200
    echo "--- events of kagent"
    kc -n kagent get events --sort-by=.lastTimestamp | tail -n 30
    echo "--- the controller's log"
    kc -n kagent logs deployment/kagent-controller --tail=80
    echo "--- the agent's log"
    kc -n kagent logs deployment/hello --tail=80
  } >&2 || true
}

version=$(pin kagent010_version)

# --- 1. the cluster -----------------------------------------------------------------------------------
if kind get clusters 2>/dev/null | grep -qx "$cluster"; then
  echo "ok   kind cluster $cluster exists"
else
  kind create cluster --config "$kdir/kind-config-0.10.yaml" --image "$(pin kind_node_image)" --wait 120s
fi
kc get --raw=/readyz >/dev/null

# --- 2. kagent ----------------------------------------------------------------------------------------
crds=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent-crds "$version" kagent010_crds_digest)
chart=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent "$version" kagent010_digest)
hc upgrade --install kagent-crds "$crds" --namespace kagent --create-namespace --wait --timeout 5m --set kmcp.enabled=false
hc upgrade --install kagent "$chart" --namespace kagent --values "$kdir/kagent-values-0.10.yaml" --wait --timeout 8m ||
  { diagnose "kagent did not become ready"; exit 1; }
# The chart has no key for a nodePort: the one the kind config maps to the host (18093) and the card advertises (a2aBaseUrl).
kc -n kagent patch service kagent-controller -p '{"spec":{"ports":[{"port":8083,"nodePort":30083}]}}'
kc -n kagent rollout status deployment/kagent-controller --timeout=180s

# --- 3. the model, the agent -----------------------------------------------------------------------------
kc -n kagent create configmap mock-model-mappings --from-file="$kdir/wiremock/mappings" --dry-run=client -o yaml | kc apply -f -
kc apply -f "$kdir/manifests-0.10.yaml"
kc -n kagent rollout status deployment/mock-model --timeout=180s

echo "waiting for the agent kagent/hello to be Ready (the controller makes its Deployment from the runtime image)"
ready=
for _ in $(seq 1 60); do
  ready=$(kc -n kagent get agents.kagent.dev hello -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)
  [ "$ready" = True ] && break
  sleep 5
done
if [ "$ready" != True ]; then
  diagnose "the agent kagent/hello is not Ready after 5 minutes (Ready=${ready:-unset})"
  exit 1
fi
echo "ok   agent kagent/hello is Ready"

card=http://127.0.0.1:18093/api/a2a/kagent/hello/.well-known/agent-card.json
for _ in $(seq 1 30); do
  if curl -fsS --max-time 5 -o "$tmp/card.json" "$card" 2>/dev/null; then
    echo "ok   the card is served: $card ($(jq -r '.name // "no name"' "$tmp/card.json"))"
    exit 0
  fi
  sleep 2
done
diagnose "the card of kagent/hello is not served at $card"
exit 1
