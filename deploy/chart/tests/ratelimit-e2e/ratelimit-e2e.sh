#!/bin/sh
# The kind end-to-end of the chart's limit per client address (issue #284, deploy/chart/README.md "A limit per client at the edge"): does the `Middleware` that
# `ingress.rateLimit` renders, named on the chart's own `Ingress` by `traefik.ingress.kubernetes.io/router.middlewares`, limit each client address in a real
# Traefik, and only that address? The chart's two files are applied as the chart renders them (`helm template -s templates/ingress.yaml -s
# templates/ingress-ratelimit.yaml`, the values of tests/ratelimit-e2e/values.yaml over examples/netcup.values.yaml); Traefik is the pinned chart and image of
# UPSTREAM; the backend is the pinned traefik/whoami; the clients are three pods with addresses of their own.
#
#   sh deploy/chart/tests/ratelimit-e2e/ratelimit-e2e.sh           (from the repository root, kubectl pointed at a kind cluster)
#   sh deploy/chart/tests/ratelimit-e2e/ratelimit-e2e.sh render    (helm only: prints the objects the script would apply, and checks them)
#
# The bucket of the test is small so that a few requests fill it: 1 request per 10 s, burst 5 (the chart's documented starting point, 20 per second and burst 50,
# is the README's). A refused request is Traefik's 429. What it does, in order, and asserts as it goes:
#   1. the Traefik chart of UPSTREAM downloaded, its SHA-256 compared with the pin, and installed with the image pinned by digest and the entrypoints trusting
#      X-Forwarded-For from the pods (`forwardedHeaders.trustedIPs`); three client pods (the pinned curl) and a backend (the pinned whoami) behind the Service
#      the chart's Ingress names; a self-signed certificate for the test host, in the Secret the Ingress names; a second middleware of the "deployer",
#      chained after ours, that adds a response header;
#   2. the control, the limit OFF (the chart's default): the Ingress names only the deployer's middleware; twelve requests from client A are all 200 and carry
#      the deployer's header, so every 429 below comes from the chart's middleware and from nothing else;
#   3. the limit ON at `sourceDepth: 0` (the address of the peer): once Traefik has the new Ingress (told by a third client, C, which has a bucket of its own),
#      client A sends twelve requests: the first five are 200, the last is 429, and the 429 has Retry-After and not the deployer's header (ours runs first);
#      client B, a second pod, is 200 three times while A is refused (one bucket for each address); A is refused again with an X-Forwarded-For of its own
#      choosing (at depth 0 the header is not the source);
#   4. `sourceDepth: 1` (the last address of X-Forwarded-For): only the Middleware is applied again; once it is live (C, sending a different forwarded address
#      each time, is never refused), A with the forwarded address X is refused after five, A with another forwarded address Y is 200 (two addresses from one pod
#      are two clients), B with the address X is refused (the bucket is the forwarded address, not the pod); and requests with no X-Forwarded-For all share one
#      bucket (A fills it, B is refused): the failure the README warns of, a limit with the wrong address throttling everybody together.
#
# What it does NOT prove: the netcup Traefik (its version, whether it has `providers.kubernetesCRD.safeNaming`, which changes the name the annotation must
# carry, its `forwardedHeaders.trustedIPs`, what X-Forwarded-For holds behind its load balancer: open question 51), the rate of the starting point under real
# traffic, two Traefik replicas (each keeps its own buckets), an SSE stream (a request is counted when it arrives, by Traefik's design: read in its source,
# not run here), or the Ingress's TLS through cert-manager (the certificate here is self-signed and curl does not verify it).
#
# Safety. The script installs and uninstalls a Helm release, applies CRDs and deletes namespaces, so it runs only on a throwaway cluster and touches only what
# it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); RATELIMIT_E2E_ALLOW_ANY_CONTEXT=1 says another cluster is a throwaway one too. Checked
#     before anything is made and before the cleanup is armed.
#   * it refuses a cluster that has Traefik's CRDs already (it would install over somebody's Traefik and delete the CRDs afterwards) and a namespace of its names
#     that exists. The namespaces are RATELIMIT_E2E_NAMESPACE (default ratelimit-e2e: the backend, the clients, the Ingress) and <it>-traefik, made with the label
#     another-agentic-e2e/made-by=ratelimit-e2e.
#   * the cleanup undoes what the run got as far as making, each step recorded when it begins: the namespaces (with the Ingress and the middlewares in them), the
#     Helm release, and the CRDs that release installed (none was there before). A run that stops before any of that deletes and uninstalls nothing.
#
# Needs kubectl, helm (3.19), jq, curl, openssl, tar and sha256sum (or shasum); a cluster that reaches docker.io. Every wait is bounded; on a failure the
# state of the cluster is printed (the test's private key is never printed). Environment (all optional): RATELIMIT_E2E_NAMESPACE,
# RATELIMIT_E2E_ALLOW_ANY_CONTEXT, HELM_TIMEOUT (default 5m), LIVE_TIMEOUT_SECS (how long Traefik may take to apply a changed object, default 90).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
chart=$repo/deploy/chart
upstream=$here/UPSTREAM

# printf, never echo, for anything that can hold a body: dash's echo interprets backslashes.
step() { printf '\n== %s\n' "$1"; }
ok() { printf 'ok   %s\n' "$*"; }
note() { printf '     %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# pin KEY: the value of a `key=value` line of UPSTREAM
pin() {
  _v=$(sed -n "s/^$1=//p" "$upstream" | head -n 1)
  [ -n "$_v" ] || die "$upstream has no line $1="
  printf '%s' "$_v"
}

ns=${RATELIMIT_E2E_NAMESPACE:-ratelimit-e2e}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "RATELIMIT_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac
[ "${#ns}" -le 40 ] || die "RATELIMIT_E2E_NAMESPACE='$ns' is longer than 40 characters"
ns_tr=$ns-traefik

release=another-agentic-system
mw_key='traefik.ingress.kubernetes.io/router.middlewares'
deployer_mw=e2e-deployer-header
deployer_header='X-E2E-Deployer'
host=ratelimit.e2e.test
# The bucket of the test: 1 per 10 s with a burst of 5. At this rate Traefik delays nothing (the delay it would add is capped at half a second) and refuses
# the sixth request of a burst, so what a client sees is five 200s and then 429s.
average=1
period=10s
burst=5

# render_chart <off | DEPTH>: the chart's objects with the values of this directory: the Ingress alone with the limit off (the chart renders no Middleware then,
# and Helm refuses --show-only for a template that renders nothing), the Ingress and the Middleware with it on, at that depth. The deployer's own middleware
# is chained through ingress.annotations, as a deployment would, and names this run's namespace.
render_chart() {
  if [ "$1" = off ]; then
    set -- --set ingress.rateLimit.enabled=false
  else
    set -- --set ingress.rateLimit.enabled=true --set ingress.rateLimit.sourceDepth="$1" --show-only templates/ingress-ratelimit.yaml
  fi
  helm template "$release" "$chart" --namespace "$ns" \
    -f "$chart/examples/netcup.values.yaml" -f "$here/values.yaml" \
    --set-json "ingress.annotations={\"$mw_key\":\"$ns-$deployer_mw@kubernetescrd\"}" \
    --set ingress.rateLimit.average=$average --set-string ingress.rateLimit.period=$period --set ingress.rateLimit.burst=$burst \
    --show-only templates/ingress.yaml "$@"
}

# doc_of KIND FILE: the YAML document of that kind (with its apiVersion), from a render.
doc_of() {
  awk -v k="$1" '
    function flush() { if (buf ~ ("(^|\n)kind: " k "\n")) printf "%s", buf; buf = "" }
    /^---$/ { flush(); next }
    { buf = buf $0 "\n" }
    END { flush() }' "$2"
}

# annotation_of FILE: the value of the Ingress's router.middlewares annotation
annotation_of() { sed -n "s|^    $mw_key: \\(.*\\)\$|\\1|p" "$1"; }

# check_render FILE ENABLED DEPTH: what the rest of the script depends on is in the render (sets backend, backend_port, tls_secret, mw_name).
check_render() {
  _f=$1; _on=$2; _depth=$3
  [ "$(grep -c '^kind: Ingress$' "$_f")" = 1 ] || die "the render is not exactly one Ingress"
  grep -q "^    - host: \"$host\"\$" "$_f" || die "the Ingress is not for the host $host"
  backend=$(awk '/^ *service:$/ { s = 1; next } s && /^ *name: / { print $2; exit }' "$_f")
  [ -n "$backend" ] || die "the render has no backend service"
  backend_port=$(awk '/^ *service:$/ { s = 1; next } s && /^ *port:$/ { p = 1; next } p && /^ *name: / { print $2; exit }' "$_f")
  [ "$backend_port" = http ] || die "the Ingress's backend port is '$backend_port', not the named port http this script's Service has"
  tls_secret=$(sed -n 's/^      secretName: //p' "$_f")
  [ -n "$tls_secret" ] || die "the Ingress has no TLS Secret name"
  _ann=$(annotation_of "$_f")
  if [ "$_on" = 0 ]; then
    [ "$(grep -c '^kind: Middleware$' "$_f")" = 0 ] || die "the limit is off and the render has a Middleware"
    [ "$_ann" = "$ns-$deployer_mw@kubernetescrd" ] || die "the limit is off and the Ingress's middlewares are '$_ann', not the deployer's alone"
    mw_name=''
  else
    [ "$(grep -c '^kind: Middleware$' "$_f")" = 1 ] || die "the limit is on and the render is not exactly one Middleware"
    mw_name=$(awk '/^kind: Middleware$/ { m = 1 } m && /^  name: / { print $2; exit }' "$_f")
    [ -n "$mw_name" ] || die "the Middleware has no name"
    [ "$_ann" = "$ns-$mw_name@kubernetescrd,$ns-$deployer_mw@kubernetescrd" ] \
      || die "the Ingress's middlewares are '$_ann', not ours then the deployer's: $ns-$mw_name@kubernetescrd,$ns-$deployer_mw@kubernetescrd"
    grep -Eq "^    average: $average\$" "$_f" || die "the Middleware's average is not $average"
    grep -Eq "^    period: \"$period\"\$" "$_f" || die "the Middleware's period is not $period"
    grep -Eq "^    burst: $burst\$" "$_f" || die "the Middleware's burst is not $burst"
    grep -Eq "^        depth: $_depth\$" "$_f" || die "the Middleware's depth is not $_depth"
  fi
}

mode=${1:-run}
case $mode in
  run | render) ;;
  *) die "usage: ratelimit-e2e.sh [render]" ;;
esac

if [ "$mode" = render ]; then
  command -v helm >/dev/null 2>&1 || die "helm is not installed"
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  render_chart off > "$work/off.yaml"
  check_render "$work/off.yaml" 0 0
  ok "limit off: the Ingress names the deployer's middleware alone, and no Middleware is made" >&2
  render_chart 0 > "$work/on0.yaml"
  check_render "$work/on0.yaml" 1 0
  ok "limit on, depth 0: the Middleware ($mw_name) and an Ingress that names it, then the deployer's, in this order" >&2
  render_chart 1 > "$work/on1.yaml"
  check_render "$work/on1.yaml" 1 1
  ok "limit on, depth 1: the same Ingress as at depth 0 (only the Middleware moves)" >&2
  [ "$(doc_of Ingress "$work/on0.yaml" | cksum)" = "$(doc_of Ingress "$work/on1.yaml" | cksum)" ] \
    || die "the Ingress differs between depth 0 and depth 1: the script applies only the Middleware for the second"
  cat "$work/on1.yaml"
  exit 0
fi

# ---- before anything is made, and before the cleanup exists: the tools, and a cluster that is a throwaway one -------------------------------------
for t in kubectl helm jq curl openssl tar; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
if command -v sha256sum >/dev/null 2>&1; then
  sha256_of() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256_of() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  die "neither sha256sum nor shasum is installed"
fi
context=$(kubectl config current-context 2>/dev/null) || context=''
[ -n "$context" ] || die "kubectl has no current context"
case $context in
  kind-*) ;;
  *)
    [ "${RATELIMIT_E2E_ALLOW_ANY_CONTEXT:-}" = 1 ] \
      || die "kubectl's current context is '$context', not a kind cluster's (kind-<name>): this script installs a Helm release, applies CRDs and deletes namespaces, so it will not run against it. RATELIMIT_E2E_ALLOW_ANY_CONTEXT=1 says it is a throwaway cluster"
    printf '::warning::%s\n' "kubectl's context '$context' is not a kind cluster's: RATELIMIT_E2E_ALLOW_ANY_CONTEXT=1 says it is a throwaway one"
    ;;
esac
# helm resolves its context by itself: the one that was just checked is the one it uses.
export HELM_KUBECONTEXT="$context"
k() { kubectl --context "$context" "$@"; }

made_by_key='another-agentic-e2e/made-by'
made_by='ratelimit-e2e'
crd_pattern='\.traefik\.io$|\.traefik\.containo\.us$'

k get --raw=/readyz >/dev/null 2>&1 || die "the cluster of the context '$context' does not answer"
existing=$(k get crd -o name 2>/dev/null | grep -E "$crd_pattern" || true)
if [ -n "$existing" ]; then
  die "the cluster has Traefik's CRDs already ($(printf '%s' "$existing" | head -n 3 | tr '\n' ' ')...): this script installs Traefik itself and deletes those CRDs at the end, so it will not run over somebody's. Use a fresh cluster"
fi
for n in "$ns" "$ns_tr"; do
  if k get namespace "$n" >/dev/null 2>&1; then
    die "the namespace $n exists: a leftover of an earlier run (kubectl delete namespace $n, if it has the label $made_by_key=$made_by) or somebody's"
  fi
done

traefik_chart_sha256=$(pin traefik_chart_sha256)
traefik_image_digest=$(pin traefik_image_digest)
traefik_version=$(pin traefik_version)
whoami_image=$(pin whoami_image)
curl_image=$(pin curl_image)
helm_timeout=${HELM_TIMEOUT:-5m}
live_timeout=${LIVE_TIMEOUT_SECS:-90}

work=$(mktemp -d)
umask 077

# What this run has made, recorded when it begins: the cleanup undoes these and nothing else.
ns_made=0       # the namespaces
traefik_installed=0

diagnostics() {
  echo "::group::what the cluster says"
  k -n "$ns" get pods,svc,ingress,middlewares.traefik.io -o wide 2>&1 || true
  k -n "$ns" get ingress -o yaml 2>&1 | tail -n 60 || true
  k -n "$ns" get middlewares.traefik.io -o yaml 2>&1 | tail -n 80 || true
  k -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | tail -n 40 || true
  echo "::endgroup::"
  echo "::group::traefik"
  k -n "$ns_tr" get pods,svc -o wide 2>&1 || true
  k -n "$ns_tr" logs deployment/traefik --tail=150 2>&1 || true
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  if [ "$ns_made" = 1 ]; then
    # The Ingress and the middlewares go with the namespace; Traefik is still there to see them go.
    if k get namespace "$ns" -o json 2>/dev/null | jq -e --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels[$k] == $v' >/dev/null 2>&1; then
      k delete namespace "$ns" --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
    fi
  fi
  if [ "$traefik_installed" = 1 ]; then helm uninstall traefik --namespace "$ns_tr" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$ns_made" = 1 ]; then
    if k get namespace "$ns_tr" -o json 2>/dev/null | jq -e --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels[$k] == $v' >/dev/null 2>&1; then
      k delete namespace "$ns_tr" --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
    fi
  fi
  if [ "$traefik_installed" = 1 ]; then
    # The chart keeps its CRDs when it is uninstalled; none was in the cluster before this run (the run refused otherwise), so they are this run's.
    for crd in $(k get crd -o name 2>/dev/null | grep -E "$crd_pattern" || true); do
      k delete "$crd" --ignore-not-found --wait=false >/dev/null 2>&1 || true
    done
  fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# dash (sh on the runners) runs the EXIT trap on `exit` and not when a signal ends the script: each signal becomes an exit, with the usual status.
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# until_ok <seconds> <description> <command...>: poll every 2 s; the output of the last attempt is what the failure shows.
until_ok() {
  _limit=$1; _what=$2; shift 2
  _deadline=$(( $(date +%s) + _limit ))
  while ! last=$("$@" 2>&1); do
    [ "$(date +%s)" -lt "$_deadline" ] || die "gave up after ${_limit}s waiting for: $_what. The last attempt said: $(printf '%s' "$last" | tail -c 900)"
    sleep 2
  done
}

step "0. the Traefik chart, with the checksum of UPSTREAM, and the chart's objects"
mkdir -p "$work/dl"
curl -fsSL --retry 3 --retry-delay 3 -o "$work/dl/traefik.tgz" "$(pin traefik_chart_url)" || die "cannot download $(pin traefik_chart_url)"
got=$(sha256_of "$work/dl/traefik.tgz")
[ "$got" = "$traefik_chart_sha256" ] || die "the Traefik chart has SHA-256 $got, $upstream pins $traefik_chart_sha256: the file moved or the pin is wrong"
ok "traefik.tgz is $got"
render_chart off > "$work/off.yaml"
check_render "$work/off.yaml" 0 0
render_chart 0 > "$work/on0.yaml"
check_render "$work/on0.yaml" 1 0
render_chart 1 > "$work/on1.yaml"
check_render "$work/on1.yaml" 1 1
ok "the chart renders the Ingress ($backend, TLS Secret $tls_secret) and, with the limit on, the Middleware $mw_name"
# The Middleware alone, for the change of depth: the Ingress is the same at both depths.
[ "$(doc_of Ingress "$work/on0.yaml" | cksum)" = "$(doc_of Ingress "$work/on1.yaml" | cksum)" ] || die "the Ingress differs between depth 0 and depth 1"
doc_of Middleware "$work/on1.yaml" > "$work/mw1.yaml"
[ -s "$work/mw1.yaml" ] || die "the depth 1 render has no Middleware"
printf '%s\n' "image: {digest: $traefik_image_digest}" > "$work/traefik-values.yaml"
cat >> "$work/traefik-values.yaml" <<EOF
versionOverride: $traefik_version
service:
  spec:
    type: ClusterIP
providers:
  kubernetesIngress:
    publishedService:
      enabled: false
ports:
  web:
    forwardedHeaders:
      trustedIPs: ["0.0.0.0/0", "::/0"]
  websecure:
    forwardedHeaders:
      trustedIPs: ["0.0.0.0/0", "::/0"]
EOF

step "1. Traefik, the clients, the backend, the certificate and the deployer's middleware"
make_namespace() {
  jq -n --arg n "$1" --arg k "$made_by_key" --arg v "$made_by" '{apiVersion: "v1", kind: "Namespace", metadata: {name: $n, labels: {($k): $v}}}' | k create -f - >/dev/null
  ns_made=1
}
make_namespace "$ns"
make_namespace "$ns_tr"
ok "namespaces $ns and $ns_tr made, with the label $made_by_key=$made_by"
traefik_installed=1
helm install traefik "$work/dl/traefik.tgz" --namespace "$ns_tr" -f "$work/traefik-values.yaml" --wait --timeout "$helm_timeout" >/dev/null \
  || die "helm install of Traefik failed"
k -n "$ns_tr" rollout status deployment/traefik --timeout=120s >/dev/null || die "Traefik did not roll out"
img=$(k -n "$ns_tr" get deployment traefik -o jsonpath='{.spec.template.spec.containers[0].image}')
case $img in *"@$traefik_image_digest") ok "Traefik $traefik_version runs $img" ;; *) die "Traefik runs $img, not the digest of UPSTREAM" ;; esac
tip=$(k -n "$ns_tr" get svc traefik -o jsonpath='{.spec.clusterIP}')
[ -n "$tip" ] || die "Traefik's Service has no cluster address"
ok "Traefik's Service is $tip (ClusterIP: a pod's own address reaches Traefik unchanged)"

openssl req -x509 -newkey rsa:2048 -nodes -keyout "$work/tls.key" -out "$work/tls.crt" -days 1 -subj "/CN=$host" -addext "subjectAltName=DNS:$host" >/dev/null 2>&1 \
  || die "openssl could not make the certificate"
k -n "$ns" create secret tls "$tls_secret" --cert "$work/tls.crt" --key "$work/tls.key" >/dev/null
for c in a b c; do
  cat <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: client-$c
  labels: {app: client}
spec:
  terminationGracePeriodSeconds: 0
  containers:
    - name: curl
      image: $curl_image
      command: ["sleep", "3600"]
EOF
  printf -- '---\n'
done > "$work/clients.yaml"
cat >> "$work/clients.yaml" <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: whoami
spec:
  replicas: 1
  selector: {matchLabels: {app: whoami}}
  template:
    metadata:
      labels: {app: whoami}
    spec:
      containers:
        - name: whoami
          image: $whoami_image
          ports: [{containerPort: 80}]
---
apiVersion: v1
kind: Service
metadata:
  name: $backend
spec:
  selector: {app: whoami}
  ports:
    - name: http
      port: 80
      targetPort: 80
---
apiVersion: traefik.io/v1alpha1
kind: Middleware
metadata:
  name: $deployer_mw
spec:
  headers:
    customResponseHeaders:
      $deployer_header: chained
EOF
k -n "$ns" apply -f "$work/clients.yaml" >/dev/null
k -n "$ns" wait --for=condition=Ready pod/client-a pod/client-b pod/client-c --timeout=180s >/dev/null || die "the client pods did not become ready"
k -n "$ns" rollout status deployment/whoami --timeout=180s >/dev/null || die "the backend did not roll out"
ok "three client pods, the backend behind $backend, the deployer's middleware $deployer_mw"

# fire POD N [curl args...]: N requests from the pod to the host through Traefik (https, the certificate not checked, the name resolved to Traefik's Service
# so that the SNI is the host); prints the status of each, ending with a space.
fire() {
  _pod=$1; _n=$2; shift 2
  # shellcheck disable=SC2016 # the loop is for the pod's shell, not this one
  k -n "$ns" exec "$_pod" -- sh -c '
    n=$1; shift
    i=0
    while [ "$i" -lt "$n" ]; do
      curl -sk --max-time 10 -o /dev/null -w "%{http_code} " "$@"
      i=$((i + 1))
    done' sh "$_n" --resolve "$host:443:$tip" "$@" "https://$host/"
}
# head_of POD [curl args...]: the response headers of one request.
head_of() {
  _pod=$1; shift
  k -n "$ns" exec "$_pod" -- curl -sk --max-time 10 -D - -o /dev/null --resolve "$host:443:$tip" "$@" "https://$host/"
}
# header_has HEADERS NAME: the response has the header (case does not matter).
header_has() { printf '%s' "$1" | tr -d '\r' | grep -qi "^$2:"; }
# first_n_then_refused LABEL STATUSES N TOTAL: the first N statuses are 200, the last is 429, and there are at most N+1 200s in all (a token may be refilled
# in the seconds the loop takes: the rate is one in ten).
first_n_then_refused() {
  _label=$1; _s=$2; _n=$3; _total=$4
  _count=$(printf '%s' "$_s" | wc -w | tr -d ' ')
  [ "$_count" = "$_total" ] || die "$_label: $_count statuses ('$_s'), want $_total"
  _i=1
  while [ "$_i" -le "$_n" ]; do
    [ "$(printf '%s' "$_s" | cut -d ' ' -f "$_i")" = 200 ] || die "$_label: request $_i is not 200: '$_s'"
    _i=$((_i + 1))
  done
  [ "$(printf '%s' "$_s" | cut -d ' ' -f "$_total")" = 429 ] || die "$_label: the last request is not 429: '$_s'"
  _ok=$(printf '%s' "$_s" | tr ' ' '\n' | grep -c '^200$' || true)
  [ "$_ok" -le $((_n + 1)) ] || die "$_label: $_ok requests were 200, the bucket holds $_n: '$_s'"
  ok "$_label: $_s"
}
all_are() { # all_are LABEL STATUSES CODE N
  _t=''
  _i=0
  while [ "$_i" -lt "$4" ]; do _t="$_t$3 "; _i=$((_i + 1)); done
  [ "$2" = "$_t" ] || die "$1: '$2', want '$_t'"
  ok "$1: $2"
}

step "2. the control: the limit is OFF (the chart's default)"
k -n "$ns" apply -f "$work/off.yaml" >/dev/null
until_ok "$live_timeout" "Traefik to route $host to the backend" sh -c "[ \"\$(kubectl --context '$context' -n '$ns' exec client-c -- curl -sk --max-time 5 -o /dev/null -w '%{http_code}' --resolve '$host:443:$tip' 'https://$host/')\" = 200 ]"
ok "Traefik routes https://$host/ to the backend"
all_are "limit off: twelve requests from client A" "$(fire client-a 12)" 200 12
hdr=$(head_of client-a)
header_has "$hdr" "$deployer_header" || die "limit off: the response has no $deployer_header: the deployer's middleware is not in the chain. Got: $hdr"
ok "limit off: the deployer's middleware ran ($deployer_header on the response)"

step "3. the limit ON, sourceDepth 0 (the address of the peer)"
k -n "$ns" apply -f "$work/on0.yaml" >/dev/null
# C has a bucket of its own and is not used by the assertions: sending until it is refused tells that Traefik has the new Ingress and Middleware.
live_refused() { case $(fire client-c 8) in *429*) return 0 ;; *) echo "client C was not refused yet"; return 1 ;; esac; }
until_ok "$live_timeout" "Traefik to apply the limit (a client sending 8 requests in a row to be refused)" live_refused
ok "Traefik applies the Middleware $mw_name"
first_n_then_refused "depth 0: client A, twelve requests" "$(fire client-a 12)" "$burst" 12
hdr=$(head_of client-a)
printf '%s' "$hdr" | head -n 1 | grep -q ' 429' || die "depth 0: client A is not refused again: $hdr"
header_has "$hdr" retry-after || die "depth 0: the 429 has no Retry-After. Got: $hdr"
if header_has "$hdr" "$deployer_header"; then die "depth 0: the 429 carries the deployer's header: the deployer's middleware ran before ours"; fi
ok "depth 0: the 429 has Retry-After and not $deployer_header (ours is first in the chain)"
all_are "depth 0: client B, three requests, while A is refused" "$(fire client-b 3)" 200 3
hdr=$(head_of client-b)
header_has "$hdr" "$deployer_header" || die "depth 0: client B's 200 has no $deployer_header: the chain stops at ours. Got: $hdr"
ok "depth 0: a 200 passes ours and then the deployer's middleware"
all_are "depth 0: client A, forging X-Forwarded-For and X-Real-Ip, is still refused (at depth 0 the headers are not the source)" \
  "$(fire client-a 2 -H 'X-Forwarded-For: 203.0.113.98' -H 'X-Real-Ip: 203.0.113.98')" 429 2

step "4. sourceDepth 1 (the last address of X-Forwarded-For): only the Middleware changes"
k -n "$ns" apply -f "$work/mw1.yaml" >/dev/null
# Live when C, sending a different forwarded address each time, is never refused (at depth 0 its own address's bucket is empty since step 3).
# (until_ok runs this in a subshell, so the addresses come from the clock: a new third octet each attempt, two seconds apart at least.)
live_depth1() {
  _t=$(( $(date +%s) % 250 ))
  _r=''
  _j=1
  while [ "$_j" -le 8 ]; do
    _r="$_r$(fire client-c 1 -H "X-Forwarded-For: 198.51.$_t.$_j")"
    _j=$((_j + 1))
  done
  case $_r in *429*) echo "client C was still refused: $_r"; return 1 ;; esac
}
until_ok "$live_timeout" "Traefik to apply depth 1 (a client with a new forwarded address each time not to be refused)" live_depth1
ok "Traefik applies depth 1"
first_n_then_refused "depth 1: client A as 203.0.113.10, twelve requests" "$(fire client-a 12 -H 'X-Forwarded-For: 203.0.113.10')" "$burst" 12
all_are "depth 1: client A, the same pod, as 203.0.113.11: its own bucket" "$(fire client-a 3 -H 'X-Forwarded-For: 203.0.113.11')" 200 3
all_are "depth 1: client B as 203.0.113.10 is refused (the forwarded address is the bucket, not the pod)" "$(fire client-b 2 -H 'X-Forwarded-For: 203.0.113.10')" 429 2
all_are "depth 1: client B as 203.0.113.12 (another pod, a new address) is 200" "$(fire client-b 2 -H 'X-Forwarded-For: 203.0.113.12')" 200 2
all_are "depth 1: the last address of the list is the client: a fresh address first and A's exhausted one last is refused" \
  "$(fire client-b 2 -H 'X-Forwarded-For: 198.51.100.200, 203.0.113.10')" 429 2
all_are "depth 1: ... and A's exhausted address first and a fresh one last is 200" \
  "$(fire client-b 2 -H 'X-Forwarded-For: 203.0.113.10, 198.51.100.201')" 200 2
first_n_then_refused "depth 1: requests with no X-Forwarded-For share one bucket: client A fills it" "$(fire client-a 8)" "$burst" 8
all_are "depth 1: ... and client B, another pod with no header, is refused: a limit with the wrong address throttles everybody together" "$(fire client-b 2)" 429 2

step "5. depth 1 behind a Traefik that does not trust the pods (the failure the README warns of)"
# The entrypoint's forwardedHeaders.trustedIPs is emptied: Traefik now drops X-Forwarded-For from every peer, so at depth 1 no request has a source and every
# client is one address. This is what a limit at the wrong depth, or behind a proxy Traefik is not told to trust, does.
printf '%s\n' 'ports: {web: {forwardedHeaders: {trustedIPs: []}}, websecure: {forwardedHeaders: {trustedIPs: []}}}' > "$work/traefik-untrusted.yaml"
helm upgrade traefik "$work/dl/traefik.tgz" --namespace "$ns_tr" -f "$work/traefik-values.yaml" -f "$work/traefik-untrusted.yaml" --wait --timeout "$helm_timeout" >/dev/null \
  || die "helm upgrade of Traefik failed"
k -n "$ns_tr" rollout status deployment/traefik --timeout=120s >/dev/null || die "Traefik did not roll out again"
k -n "$ns_tr" get deployment traefik -o jsonpath='{.spec.template.spec.containers[0].args}' | grep -q 'forwardedHeaders.trustedIPs' \
  && die "Traefik still has trustedIPs after the upgrade"
tip=$(k -n "$ns_tr" get svc traefik -o jsonpath='{.spec.clusterIP}')
routed() { case $(fire client-c 1) in "200 "|"429 ") return 0 ;; *) echo "not routed yet"; return 1 ;; esac; }
until_ok "$live_timeout" "the new Traefik to route $host" routed
ok "Traefik no longer trusts the pods' X-Forwarded-For"
# Each request has an address of its own in the header, and the bucket is the same one for all of them. (C has used one of its tokens to find out above.)
mixed=''
for n in 1 2 3 4 5 6 7 8; do mixed="$mixed$(fire client-a 1 -H "X-Forwarded-For: 203.0.113.$n")"; done
# (C spent one token of the shared bucket to find out, and a token comes back every ten seconds: four to six 200s, and the last request refused.)
n200=$(printf '%s' "$mixed" | tr ' ' '\n' | grep -c '^200$' || true)
last_code=$(printf '%s' "$mixed" | awk '{ print $NF }')
if [ "$n200" -lt 4 ] || [ "$n200" -gt 6 ] || [ "$last_code" != 429 ]; then
  die "untrusted, depth 1: eight requests of eight forwarded addresses were '$mixed': want four to six 200s and 429 at the end, the bucket of one address"
fi
ok "untrusted, depth 1: eight requests of eight forwarded addresses share one bucket: $mixed"
all_are "untrusted, depth 1: client B, with an address of its own in the header, is refused too" "$(fire client-b 2 -H 'X-Forwarded-For: 198.51.100.7')" 429 2

step "done"
ok "the chart's Middleware limits each client address in Traefik $traefik_version: a bucket for each peer at depth 0, for each forwarded address at depth 1, one shared bucket when the header is missing or not trusted"
