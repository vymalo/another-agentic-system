#!/usr/bin/env sh
# Brings up what dev/kagent-e2e.sh needs on the Kubernetes side (dev/README.md, "kagent"): a kind cluster, Agent Substrate, kagent 1.x
# and one agent, `kagent/hello`, on a scripted OpenAI-compatible model. Safe to run again: every step is an upgrade or an apply.
#
#   dev/kagent/up.sh          # about 5 to 10 minutes the first time (the images are about 1.2 GB compressed)
#   dev/kagent/down.sh        # deletes the cluster (and with it everything above)
#
# kagent 1.x runs each agent as an Actor of Agent Substrate (https://github.com/kagent-dev/substrate), so this is kagent's own CI recipe
# (.github/workflows/ci.yaml of kagent v1.0.0-alpha8, job test-e2e) with three changes: the images and charts are the published ones, no local
# registry and no MetalLB (the controller is a NodePort the kind config maps), and every pin is in dev/kagent/UPSTREAM.
#
#   1. a kind cluster `kagent` (dev/kagent/kind-config.yaml: the feature gates Substrate needs, two NodePorts on the host's loopback);
#   2. Substrate: its CRDs and chart, then the CA pools, the actor-identity JWT pool and the authentication ConfigMap its API needs
#      (`kubectl-ate admin ...`, as kagent's CI does), then the rollout;
#   3. kagent: the CRDs chart and the chart, with dev/kagent/kagent-values.yaml (no UI, no tool servers, a WorkerPool of two gVisor workers);
#   4. dev/kagent/manifests.yaml: the scripted model (WireMock, with dev/kagent/wiremock/mappings), a ModelConfig, a Harness, an
#      AgentTemplate and an Agent; and it waits for the agent to be Ready and its card to be served.
#
# A chart is pulled by version and its digest is compared with the one in dev/kagent/UPSTREAM first: a different digest stops the script.
#
# Needs on the PATH: docker, kind, kubectl, helm, jq, curl, openssl, base64 (CI installs kind, kubectl and helm at pinned versions with their checksums,
# and `kubectl-ate` is downloaded here, with its checksum, unless it is on the PATH). The host needs no KVM (the gVisor sandbox class is used), and
# on Ubuntu 24.04 `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` first if a sandbox cannot start.
# It leaves a cluster running and docker pulls behind: dev/kagent/down.sh removes the cluster, `docker image rm` the node image.
#
# Environment: KAGENT_DIAG=0 turns the diagnostics (what failed to become ready, the controller's log) off.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
kdir=$root/dev/kagent
# shellcheck disable=SC2034 # read by pin() of lib.sh
upstream=$kdir/UPSTREAM
ctx=kind-kagent

kc() { kubectl --context "$ctx" "$@"; }
hc() { helm --kube-context "$ctx" "$@"; }

for tool in docker kind kubectl helm jq curl openssl base64; do
  command -v "$tool" >/dev/null 2>&1 || { echo "up.sh: $tool is required and was not found on the PATH" >&2; exit 2; }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# shellcheck source=dev/kagent/lib.sh
. "$kdir/lib.sh"

diagnose() { # diagnose WHAT: what is not ready, to help whoever reads the log
  echo "up.sh: $1" >&2
  [ "${KAGENT_DIAG:-1}" = 0 ] && return 0
  {
    echo "--- pods"
    kc get pods -A -o wide
    echo "--- the agent, its template and its harness"
    kc -n kagent get agents,agenttemplates,harnesses,modelconfigs -o yaml | head -n 200
    echo "--- worker pools"
    kc get workerpools -A
    echo "--- events of kagent"
    kc -n kagent get events --sort-by=.lastTimestamp | tail -n 30
    echo "--- the controller's log"
    kc -n kagent logs deployment/kagent-controller --tail=80
  } >&2 || true
}

kagent_version=$(pin kagent_version)
substrate_version=$(pin substrate_version)

# --- 1. the cluster -----------------------------------------------------------------------------------
if kind get clusters 2>/dev/null | grep -qx kagent; then
  echo "ok   kind cluster kagent exists"
else
  kind create cluster --config "$kdir/kind-config.yaml" --image "$(pin kind_node_image)" --wait 120s
fi
kc get --raw=/readyz >/dev/null

# --- 2. Agent Substrate -------------------------------------------------------------------------------
substrate_crds=$(pull_chart ghcr.io/kagent-dev/substrate/helm/substrate-crds "$substrate_version" substrate_crds_digest)
substrate=$(pull_chart ghcr.io/kagent-dev/substrate/helm/substrate "$substrate_version" substrate_digest)

if command -v kubectl-ate >/dev/null 2>&1; then
  ate=$(command -v kubectl-ate)
else
  ate=$tmp/kubectl-ate
  curl -fsSL -o "$ate" "https://github.com/kagent-dev/substrate/releases/download/v${substrate_version}/kubectl-ate-linux-amd64"
  _sum=$(sha256sum "$ate" | cut -d ' ' -f 1)
  if [ "$_sum" != "$(pin kubectl_ate_sha256)" ]; then
    echo "up.sh: kubectl-ate has sha256 $_sum, dev/kagent/UPSTREAM pins $(pin kubectl_ate_sha256)" >&2
    exit 1
  fi
  chmod +x "$ate"
fi

hc upgrade --install substrate-crds "$substrate_crds" --namespace ate-system --create-namespace
# Not --wait yet: the pools and secrets below are what the rollout waits for.
hc upgrade --install substrate "$substrate" --namespace ate-system \
  --set-string 'ateApi.extraArgs[0]=--template-resync-interval=250ms' \
  --set 'credentialProvider.namespacePolicies[0].atespace=kagent' \
  --set 'credentialProvider.namespacePolicies[0].allowedNamespaces[0]=kagent'
"$ate" --context "$ctx" admin make-ca-pool --ca-id=1 --name=service-dns-ca-pool --secret-namespace=podcertificate-controller-system
"$ate" --context "$ctx" admin make-ca-pool --ca-id=1 --name=pod-identity-ca-pool --secret-namespace=podcertificate-controller-system
"$ate" --context "$ctx" admin make-jwt-pool --key-id=1 --name=actor-id-jwt-pool --secret-namespace=ate-system
"$ate" --context "$ctx" admin make-ca-pool --ca-id=1 --name=actor-id-ca-pool --secret-namespace=ate-system
"$ate" --context "$ctx" admin make-ca-pool --ca-id=1 --name=egress-mitm-ca-pool --secret-namespace=ate-system --key-type=ECDSAP256
kc get secret actor-id-ca-pool -n ate-system -o jsonpath='{.data.pool}' | base64 --decode |
  jq -r '.CAs[0].RootCertificateDER' | base64 --decode | openssl x509 -inform der -outform pem > "$tmp/actor-id-ca.crt"
kc -n ate-system create secret generic actor-id-ca-certs --from-file=ca.crt="$tmp/actor-id-ca.crt" --dry-run=client -o yaml | kc apply -f -
# The same ConfigMap kagent's CI creates: ate-api checks actor identities against the cluster's own service-account issuer.
kc -n ate-system create configmap ate-api-authentication --dry-run=client -o yaml --from-literal=authentication.yaml='actorIdentityJWTProvider: kubernetes
jwtProviders:
- name: kubernetes
  issuer: https://kubernetes.default.svc
  audiences: [api.ate-system.svc]
  certificateAuthorityFile: /var/run/secrets/kubernetes.io/serviceaccount/ca.crt
  discoveryTokenFile: /var/run/secrets/kubernetes.io/serviceaccount/token
' | kc apply -f -
hc upgrade substrate "$substrate" --namespace ate-system --reuse-values --wait --timeout 8m ||
  { diagnose "Substrate did not become ready"; exit 1; }

# --- 3. kagent ----------------------------------------------------------------------------------------
kagent_crds=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent-crds "$kagent_version" kagent_crds_digest)
kagent=$(pull_chart ghcr.io/kagent-dev/kagent/helm/kagent "$kagent_version" kagent_digest)
hc upgrade --install kagent-crds "$kagent_crds" --namespace kagent --create-namespace --wait --timeout 5m --set kmcp.enabled=false
hc upgrade --install kagent "$kagent" --namespace kagent --values "$kdir/kagent-values.yaml" --wait --timeout 8m ||
  { diagnose "kagent did not become ready"; exit 1; }
# The chart has no key for a nodePort: the one the kind config maps to the host (18083) and the card advertises (a2aGatewayUrl).
kc -n kagent patch service kagent-controller -p '{"spec":{"ports":[{"port":8083,"nodePort":30083}]}}'
kc -n kagent rollout status deployment/kagent-controller --timeout=180s

# --- 4. the model, the agent -----------------------------------------------------------------------------
kc -n kagent create configmap mock-model-mappings --from-file="$kdir/wiremock/mappings" --dry-run=client -o yaml | kc apply -f -
kc apply -f "$kdir/manifests.yaml"
kc -n kagent rollout status deployment/mock-model --timeout=180s

echo "waiting for the agent kagent/hello to be Ready (it is compiled, prepared on a worker and snapshotted first)"
ready=
for _ in $(seq 1 120); do
  ready=$(kc -n kagent get agent hello -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)
  [ "$ready" = True ] && break
  sleep 5
done
if [ "$ready" != True ]; then
  diagnose "the agent kagent/hello is not Ready after 10 minutes (Ready=${ready:-unset})"
  exit 1
fi
echo "ok   agent kagent/hello is Ready"

card=http://127.0.0.1:18083/agents/kagent/hello/.well-known/agent-card.json
for _ in $(seq 1 30); do
  if curl -fsS --max-time 5 -o "$tmp/card.json" "$card" 2>/dev/null; then
    echo "ok   the card is served: $card ($(jq -r '.name // "no name"' "$tmp/card.json"))"
    exit 0
  fi
  sleep 2
done
diagnose "the card of kagent/hello is not served at $card"
exit 1
