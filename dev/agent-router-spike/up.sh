#!/usr/bin/env sh
# The S9 spike's cluster (sys#334 S9): Envoy Gateway v1.10.0 with Agent Router's values, Agent
# Router v1.3.0, a mock model that echoes the headers it received, and a stub external authorizer.
# Safe to run again: every step is an upgrade or an apply.
#
#   sh dev/agent-router-spike/up.sh     # about 5 minutes the first time (the images are ~1.5 GB)
#   sh dev/agent-router-spike/spike.sh  # the proof
#   sh dev/agent-router-spike/down.sh   # deletes the cluster
#
# Every chart is pulled by version and its digest compared with UPSTREAM first; a different digest
# stops the script. The images are pinned by digest in UPSTREAM and the manifests.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=dev/agent-router-spike/UPSTREAM
. "$dir/UPSTREAM"
cluster=agent-router-spike
ctx=kind-$cluster

for tool in docker kind kubectl helm curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "up.sh: $tool is required and was not found on the PATH" >&2; exit 2; }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

pull() { # pull CHART VERSION EXPECTED_DIGEST
  out=$(helm pull "$1" --version "$2" -d "$tmp" 2>&1)
  if ! printf '%s\n' "$out" | grep -q "Digest: $3"; then
    echo "up.sh: $1@$2 does not have digest $3:" >&2
    printf '%s\n' "$out" >&2
    exit 1
  fi
}

pull "$ENVOY_GATEWAY_CHART" "$ENVOY_GATEWAY_VERSION" "$ENVOY_GATEWAY_DIGEST"
pull "$AI_GATEWAY_CRDS_CHART" "$AI_GATEWAY_VERSION" "$AI_GATEWAY_CRDS_DIGEST"
pull "$AI_GATEWAY_CHART" "$AI_GATEWAY_VERSION" "$AI_GATEWAY_DIGEST"

if ! kind get clusters 2>/dev/null | grep -qx "$cluster"; then
  kind create cluster --name "$cluster" --config "$dir/kind-config.yaml"
fi
kc() { kubectl --context "$ctx" "$@"; }
hc() { helm --kube-context "$ctx" "$@"; }

hc upgrade -i eg "$ENVOY_GATEWAY_CHART" --version "$ENVOY_GATEWAY_VERSION" \
  --namespace envoy-gateway-system --create-namespace \
  -f "$dir/envoy-gateway-values.yaml"
kc wait -n envoy-gateway-system deployment/envoy-gateway --for=condition=Available --timeout=5m

hc upgrade -i aieg-crd "$AI_GATEWAY_CRDS_CHART" --version "$AI_GATEWAY_VERSION" \
  --namespace envoy-ai-gateway-system --create-namespace
hc upgrade -i aieg "$AI_GATEWAY_CHART" --version "$AI_GATEWAY_VERSION" \
  --namespace envoy-ai-gateway-system
kc wait -n envoy-ai-gateway-system deployment/ai-gateway-controller --for=condition=Available --timeout=5m

kc apply -f "$dir/manifests/"
kc wait -n default pods -l app=spike-mock --for=condition=Ready --timeout=3m
kc wait -n default pods -l app=spike-extauth --for=condition=Ready --timeout=3m
kc wait -n envoy-gateway-system pods -l gateway.envoyproxy.io/owning-gateway-name=spike --for=condition=Ready --timeout=5m

# What the controller made of the AIGatewayRoute: the spike's finding starts here.
echo "up.sh: HTTPRoutes in the cluster:"
kc get httproute -A || true
echo "up.sh: ready. Next: sh dev/agent-router-spike/spike.sh"
