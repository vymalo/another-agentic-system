#!/bin/sh
# The kind end-to-end of the database scaled out (issue #290; deploy/chart/README.md "Scaling the database"): does what the chart renders for
# `database.instances: 3` and `database.pooler.enabled` run, and does the orchestrator, started with the chart's own configuration, migrate through the database
# itself (`database.directUrl`) and carry its requests through the pooler (`database.url`)? Everything is real but the identity provider and the agent: CloudNativePG,
# its Pooler (PgBouncer), the orchestrator image the chart pins, on a kind cluster. The chart's own files are applied (`helm template -s templates/cnpg-clusters.yaml -s
# templates/cnpg-pooler.yaml -s templates/orchestrator-{configmap,deployment,service,pvc}.yaml`), with the values of tests/pooler-e2e/values.yaml.
#
#   sh deploy/chart/tests/pooler-e2e/pooler-e2e.sh           (from the repository root, kubectl pointed at a kind cluster)
#   sh deploy/chart/tests/pooler-e2e/pooler-e2e.sh render    (helm only: prints the objects the script would apply, and checks them)
#
# What it does, in order, and asserts as it goes:
#   0. the pins (UPSTREAM files), the render, and that the orchestrator image the chart pins is at or after the commit that has `database.directUrl` (git, when the
#      history is there). The render is a Cluster of 3 instances with the documented resources, a Pooler (type rw, poolMode transaction, max_prepared_statements 200) of 2
#      pods with resources, a ConfigMap whose `database.url` and `database.directUrl` are `{ env }` references, and a Deployment that composes the two URLs from the app
#      Secret's keys, with the pooler's Service host in the one and the cluster's `-rw` host in the other, mounts no `uri` file and holds no password;
#   1. the CloudNativePG operator (chart pinned by version and SHA-256 in backup-e2e/UPSTREAM), a StorageClass of the chart's default name;
#   2. the Cluster and the Pooler applied: the Cluster is Ready with 3 instances (the primary streams to two replicas, every pod has the resources asked) and the
#      Pooler's two pods are ready, with the resources asked, a Service named like it, and (when the pod lets it be read) a pgbouncer.ini in transaction mode with
#      max_prepared_statements 200; its PgBouncer is 1.21 or later;
#   3. the stand-ins: a mock OpenID Connect issuer (dev/mock-oidc) and a WireMock A2A agent (dev/wiremock/agent, the dev stack's `mock-agent`), a pod with curl;
#   4. the orchestrator: the chart's ConfigMap (with ONLY `server.environment` and `auth.jwt.issuer` changed, because the issuer is http in a cluster with no certificate
#      authority: the script asserts that those two lines are the whole difference), Service, claim and Deployment applied; a sampler on the primary records the sessions
#      of the database while it boots; the Deployment rolls out (its readiness probe is /readyz, 503 until the database answers and the issuer's keys are fetched);
#      /healthz and /readyz answer 200; the orchestrator logged that `database.directUrl` is set;
#   5. a request through the stack: a token of the mock issuer, a run for the mock agent (a thread, written), the thread ends `done`, its title is changed (a write)
#      and read back, the list holds it, the database holds the row and both replicas hold it too;
#   6. what the database and the pooler say about who connected: every session from the orchestrator's pod address is the wakeup's `LISTEN`; the sessions from the
#      pooler pods' addresses are the requests, and none of them ran a migration (sampled while it booted: nothing but the orchestrator's own address ran the
#      sqlx lock, `_sqlx_migrations` or DDL); the pooler's counters of transactions and of prepared statements for the database `orchestrator` rose; its pool mode
#      for that database is transaction (metric value 2);
#   7. the run's password and secrets are in no ConfigMap, no Deployment, no pod, no event and no log of the namespaces.
#
# What it does NOT prove: the netcup cluster (its CloudNativePG version, its storage, Argo CD), a failover (the primary is never killed), a real load or a count of
# connections under one (`default_pool_size` and `max_connections` are documented, not measured), the agents' own databases (this deployment has none), the backups
# beside three instances, the production check of the issuer (the two lines above are changed), PgBouncer versions other than the operator's default, and the
# sampling of the boot as more than evidence (a migration is a second long: when the sampler misses it the script says so and relies on the orchestrator's log line
# and on the listener, which are exact). deploy/chart/README.md "Scaling the database" says what each of those is.
#
# Safety. The script installs and uninstalls a Helm release, applies CRDs and deletes namespaces, so it runs only on a throwaway cluster and touches only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); POOLER_E2E_ALLOW_ANY_CONTEXT=1 says another cluster is a throwaway one too. Checked before
#     anything is made and before the cleanup is armed.
#   * it refuses a cluster that has CloudNativePG already (its CRDs exist): it would otherwise install over somebody's operator and delete its CRDs afterwards. Use a
#     fresh cluster. It refuses a namespace of its names that exists.
#   * the namespaces are POOLER_E2E_NAMESPACE (default pooler-e2e, everything but the operator) and <it>-operators (CloudNativePG), made with the label
#     another-agentic-e2e/made-by=pooler-e2e. The Secrets are made with random values and printed nowhere.
#   * the cleanup undoes what the run got as far as making, each step recorded when it begins: the sampler, the Pooler and the Cluster (with the operator still there),
#     the Helm release, the namespaces, the CRDs that release installed (none was there before) and the StorageClass, if the run made it. A run that stops before any of
#     that deletes and uninstalls nothing.
#
# Needs kubectl, helm (3.19), jq, curl, openssl, tar and sha256sum (or shasum); a cluster that reaches docker.io, ghcr.io, quay.io and github.com; room for three
# PostgreSQL pods that ask for 500m CPU and 1Gi each (a four-core node). Every wait is bounded; on a failure the cluster's state is printed (no line that holds one of
# the run's credentials). Environment (all optional): POOLER_E2E_NAMESPACE, POOLER_E2E_ALLOW_ANY_CONTEXT, HELM_TIMEOUT (default 8m), CLUSTER_TIMEOUT_SECS,
# POOLER_TIMEOUT_SECS, ORCHESTRATOR_TIMEOUT_SECS.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
chart=$repo/deploy/chart
upstream=$here/UPSTREAM
upstream_backup=$repo/deploy/chart/tests/backup-e2e/UPSTREAM
upstream_ratelimit=$repo/deploy/chart/tests/ratelimit-e2e/UPSTREAM

# printf, never echo, for anything that can hold a body: dash's echo interprets backslashes.
step() { printf '\n== %s\n' "$1"; }
ok() { printf 'ok   %s\n' "$*"; }
note() { printf '     %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# pin FILE KEY: the value of a `key=value` line of an UPSTREAM file
pin() {
  _v=$(sed -n "s/^$2=//p" "$1" | head -n 1)
  [ -n "$_v" ] || die "$1 has no line $2="
  printf '%s' "$_v"
}

# What the chart makes, by name (the release is called `another-agentic-system`, as examples/restore-cluster.yaml and the backup scenario assume; the chart's own
# names do not depend on it).
release=another-agentic-system
db=another-agentic-db
pooler=another-agentic-db-pooler
orch=another-agentic-orchestrator
r_cluster=clusters.postgresql.cnpg.io
r_pooler=poolers.postgresql.cnpg.io

ns=${POOLER_E2E_NAMESPACE:-pooler-e2e}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "POOLER_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac
[ "${#ns}" -le 40 ] || die "POOLER_E2E_NAMESPACE='$ns' is longer than 40 characters"
ns_ops=$ns-operators

# render_one TEMPLATE: one of the chart's files, with the values of this directory.
render_one() {
  helm template "$release" "$chart" --namespace "$ns" -f "$here/values.yaml" --show-only "templates/$1"
}

# check_render DIR: what the rest of the script depends on is in the render of the objects it applies (in DIR, one file each).
check_render() {
  _d=$1
  grep -q "^  name: $db\$" "$_d/cnpg-clusters.yaml" || die "the Cluster is not named $db"
  grep -q '^  instances: 3$' "$_d/cnpg-clusters.yaml" || die "the Cluster does not have instances: 3"
  for _l in '^      cpu: 500m$' '^      memory: 1Gi$' '^      memory: 2Gi$'; do
    grep -q "$_l" "$_d/cnpg-clusters.yaml" || die "the Cluster's resources lack the line $_l"
  done
  [ "$(grep -c '^kind: ' "$_d/cnpg-clusters.yaml")" = 1 ] || die "cnpg-clusters.yaml is not one object (the values have no chat agent and no coder, so no Database): $(grep '^kind: ' "$_d/cnpg-clusters.yaml" | tr '\n' ' ')"
  grep -q "^  name: $pooler\$" "$_d/cnpg-pooler.yaml" || die "the Pooler is not named $pooler"
  for _l in '^kind: Pooler$' '^  type: rw$' '^  instances: 2$' '^    poolMode: transaction$' '^      max_prepared_statements: "200"$' '^        - name: pgbouncer$' '^  cluster:$' "^    name: $db\$"; do
    grep -q "$_l" "$_d/cnpg-pooler.yaml" || die "the Pooler lacks the line $_l"
  done
  grep -q '^      url: { env: ORCH_DB_POOLED_URL }$' "$_d/orchestrator-configmap.yaml" || die "the ConfigMap's database.url is not { env: ORCH_DB_POOLED_URL }"
  grep -q '^      directUrl: { env: ORCH_DB_DIRECT_URL }$' "$_d/orchestrator-configmap.yaml" || die "the ConfigMap's database.directUrl is not { env: ORCH_DB_DIRECT_URL }"
  if grep -q '/run/secrets/db' "$_d/orchestrator-configmap.yaml" "$_d/orchestrator-deployment.yaml"; then die "the render still mounts or names the app Secret's uri file"; fi
  grep -qF "value: \"postgresql://\$(ORCH_DB_USER):\$(ORCH_DB_PASSWORD)@$pooler.$ns.svc:5432/\$(ORCH_DB_NAME)\"" "$_d/orchestrator-deployment.yaml" \
    || die "the Deployment's ORCH_DB_POOLED_URL is not built on the pooler's Service ($pooler.$ns.svc)"
  grep -qF "value: \"postgresql://\$(ORCH_DB_USER):\$(ORCH_DB_PASSWORD)@$db-rw.$ns.svc:5432/\$(ORCH_DB_NAME)\"" "$_d/orchestrator-deployment.yaml" \
    || die "the Deployment's ORCH_DB_DIRECT_URL is not built on the cluster's -rw Service ($db-rw.$ns.svc)"
  for _k in username password dbname; do
    grep -q "^                  key: $_k\$" "$_d/orchestrator-deployment.yaml" || die "the Deployment does not read the app Secret's key $_k"
  done
  grep -q "^                  name: $db-app\$" "$_d/orchestrator-deployment.yaml" || die "the Deployment does not read the app Secret $db-app"
  grep -q '^kind: Deployment$' "$_d/orchestrator-deployment.yaml" || die "no orchestrator Deployment"
  grep -q "^  name: $orch\$" "$_d/orchestrator-deployment.yaml" || die "the Deployment is not named $orch"
  grep -q '^            - name: CODER_A2A_TOKEN$' "$_d/orchestrator-deployment.yaml" || die "the Deployment does not read the agent's bearer CODER_A2A_TOKEN"
}

render_all() { # render_all DIR
  for _t in cnpg-clusters cnpg-pooler orchestrator-configmap orchestrator-deployment orchestrator-service orchestrator-pvc; do
    render_one "$_t.yaml" > "$1/$_t.yaml"
  done
}

mode=${1:-run}
case $mode in
  run | render) ;;
  *) die "usage: pooler-e2e.sh [render]" ;;
esac

if [ "$mode" = render ]; then
  command -v helm >/dev/null 2>&1 || die "helm is not installed"
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  render_all "$work"
  check_render "$work"
  for _t in cnpg-clusters cnpg-pooler orchestrator-configmap orchestrator-deployment orchestrator-service orchestrator-pvc; do cat "$work/$_t.yaml"; done
  ok "the render is the Cluster (3 instances, resources), the Pooler (transaction, max_prepared_statements 200) and the orchestrator's four objects, wired to the pooler and the -rw Service" >&2
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
    [ "${POOLER_E2E_ALLOW_ANY_CONTEXT:-}" = 1 ] \
      || die "kubectl's current context is '$context', not a kind cluster's (kind-<name>): this script installs a Helm release, applies CRDs and deletes namespaces, so it will not run against it. POOLER_E2E_ALLOW_ANY_CONTEXT=1 says that this cluster is a throwaway one"
    printf '::warning::%s\n' "kubectl's context '$context' is not a kind cluster's: POOLER_E2E_ALLOW_ANY_CONTEXT=1 says it is a throwaway one"
    ;;
esac
# helm resolves its context by itself: the one that was just checked is the one it uses.
export HELM_KUBECONTEXT="$context"
k() { kubectl --context "$context" "$@"; }

made_by_key='another-agentic-e2e/made-by'
made_by='pooler-e2e'
crd_pattern='\.postgresql\.cnpg\.io$'

k get --raw=/readyz >/dev/null 2>&1 || die "the cluster of the context '$context' does not answer"
existing=$(k get crd -o name 2>/dev/null | grep -E "$crd_pattern" || true)
if [ -n "$existing" ]; then
  die "the cluster has CRDs of CloudNativePG already ($(printf '%s' "$existing" | head -n 3 | tr '\n' ' ')...): this script installs the operator itself and deletes its CRDs when it ends, so it needs a cluster that has none (kind delete cluster, kind create cluster)"
fi
for n in "$ns" "$ns_ops"; do
  if k get namespace "$n" >/dev/null 2>&1; then
    die "the namespace $n exists: a leftover of an earlier run (kubectl delete namespace $n, if it has the label $made_by_key=$made_by) or somebody's"
  fi
done

# ---- the pins -------------------------------------------------------------------------------------------------------------------------------------
cnpg_chart_sha256=$(pin "$upstream_backup" cnpg_chart_sha256)
cnpg_chart_url=$(pin "$upstream_backup" cnpg_chart_url)
curl_image=$(pin "$upstream_ratelimit" curl_image)
wiremock_image=$(pin "$upstream" wiremock_image)
grep -qF "image: ${wiremock_image%@*} " "$repo/compose.yaml" || die "UPSTREAM's wiremock_image (${wiremock_image%@*}) is not the tag compose.yaml's x-wiremock runs: move them together"
node_image=$(sed -n 's/^FROM \(node:[^ ]*@sha256:[0-9a-f]*\).*$/\1/p' "$repo/dev/mock-oidc/Dockerfile" | head -n 1)
[ -n "$node_image" ] || die "dev/mock-oidc/Dockerfile has no FROM line with a node image by tag and digest"

helm_timeout=${HELM_TIMEOUT:-8m}
cluster_timeout=${CLUSTER_TIMEOUT_SECS:-720}
pooler_timeout=${POOLER_TIMEOUT_SECS:-300}
orchestrator_timeout=${ORCHESTRATOR_TIMEOUT_SECS:-420}

work=$(mktemp -d)
umask 077
render_all "$work"
check_render "$work"
orch_image=$(sed -n 's/^ *image: "\(.*orchestrator:sha-[0-9a-f]*\)"$/\1/p' "$work/orchestrator-deployment.yaml" | head -n 1)
[ -n "$orch_image" ] || die "the Deployment has no orchestrator image by a sha-<commit> tag"
ok "the chart's render is the Cluster (3 instances), the Pooler and the orchestrator's four objects; the orchestrator image is $orch_image"

step "0. does the orchestrator image the chart pins know database.directUrl?"
# The same question deploy.yml asks of other keys: is the tag's commit at or after the commit that introduced the key? The key is in docs/api/config.schema.json.
tag_sha=${orch_image##*:sha-}
intro=$(git -C "$repo" log --format=%H -S 'directUrl' -- docs/api/config.schema.json 2>/dev/null | tail -n 1 || true)
if [ -z "$intro" ]; then
  note "no history here (or no commit introduces directUrl in docs/api/config.schema.json): the check is skipped, a Deployment that exits 78 would say it in the diagnostics"
elif ! git -C "$repo" rev-parse --verify --quiet "${tag_sha}^{commit}" >/dev/null 2>&1; then
  note "the commit $tag_sha named by orchestrator.image.tag is not in this checkout (a shallow clone?): the check is skipped"
elif git -C "$repo" merge-base --is-ancestor "$intro" "$tag_sha"; then
  ok "orchestrator.image.tag (sha-$tag_sha) is at or after $(printf '%.7s' "$intro"), which introduced database.directUrl"
else
  die "orchestrator.image.tag (sha-$tag_sha) is BEFORE $(printf '%.7s' "$intro"), the commit that introduced database.directUrl: that image refuses the key (exit 78). The bump job of orchestrator.yml moves the tag after the next build of main; run this again then"
fi

# The credentials of the run: all in Secrets, and to be found nowhere else. One per line in $work/secrets, for the diagnostics and for step 8.
oidc_secret=$(openssl rand -hex 16)
thread_tools_secret=$(openssl rand -hex 32)
agent_token=$(openssl rand -hex 16)
printf '%s' "$oidc_secret" > "$work/oidc-secret"
printf '%s' "$thread_tools_secret" > "$work/thread-tools-secret"
printf '%s' "$agent_token" > "$work/agent-token"
printf '%s\n%s\n%s\n' "$oidc_secret" "$thread_tools_secret" "$agent_token" > "$work/secrets"

# What this run has made, recorded when it begins: the cleanup undoes these and nothing else.
ns_made=0       # the namespaces
crds_made=0     # the Helm install was started: the CRDs of the operator may exist (none did before)
cnpg_installed=0
sc_made=0
sampler_pid=''
storage_class=''

# redact: stdin to stdout without any line that holds one of the run's credentials, for the diagnostics.
redact() {
  if [ -s "$work/secrets" ]; then grep -vF -f "$work/secrets" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  k -n "$ns" get "$r_cluster,$r_pooler,pods,deployments,services,pvc" -o wide 2>&1 | redact || true
  k -n "$ns" get "$r_cluster/$db" -o yaml 2>&1 | redact | tail -n 110 || true
  k -n "$ns" get "$r_pooler/$pooler" -o yaml 2>&1 | redact | tail -n 60 || true
  k -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 60 || true
  echo "::endgroup::"
  echo "::group::the orchestrator (a Deployment that exits 78 says which key it refuses)"
  k -n "$ns" logs "deployment/$orch" --tail=80 2>&1 | redact || true
  k -n "$ns" logs "deployment/$orch" --previous --tail=40 2>&1 | redact || true
  if k -n "$ns" logs "deployment/$orch" --previous 2>/dev/null | grep -q 'directUrl'; then
    echo "NOTE: the orchestrator's log names directUrl: orchestrator.image.tag in deploy/chart/values.yaml is probably before the commit that introduced database.directUrl"
  fi
  echo "::endgroup::"
  echo "::group::the database pods and the pooler's pods"
  k -n "$ns" logs -l "cnpg.io/cluster=$db" --all-containers --prefix --tail=40 2>&1 | redact || true
  k -n "$ns" logs -l "cnpg.io/poolerName=$pooler" --all-containers --prefix --tail=60 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the stand-ins"
  k -n "$ns" logs deployment/oidc --tail=30 2>&1 | redact || true
  k -n "$ns" logs deployment/mock-agent --tail=30 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the operator"
  k -n "$ns_ops" get pods -o wide 2>&1 | redact || true
  k -n "$ns_ops" logs deployment/cnpg-cloudnative-pg --tail=120 2>&1 | redact || true
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ -n "$sampler_pid" ]; then kill "$sampler_pid" >/dev/null 2>&1 || true; fi
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  if [ "$ns_made" = 1 ]; then
    # While the operator is still there: the pooler, then the cluster.
    k -n "$ns" delete "$r_pooler" --all --ignore-not-found --wait=true --timeout=90s >/dev/null 2>&1 || true
    k -n "$ns" delete "$r_cluster" --all --ignore-not-found --wait=true --timeout=240s >/dev/null 2>&1 || true
  fi
  if [ "$cnpg_installed" = 1 ]; then helm uninstall cnpg --namespace "$ns_ops" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$ns_made" = 1 ]; then
    for n in "$ns" "$ns_ops"; do
      if k get namespace "$n" -o json 2>/dev/null | jq -e --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels[$k] == $v' >/dev/null 2>&1; then
        k delete namespace "$n" --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
      fi
    done
  fi
  if [ "$crds_made" = 1 ]; then
    # The chart keeps its CRDs when it is uninstalled; none was in the cluster before this run (the run refused otherwise), so they are this run's.
    for crd in $(k get crd -o name 2>/dev/null | grep -E "$crd_pattern" || true); do
      k delete "$crd" --ignore-not-found --wait=false >/dev/null 2>&1 || true
    done
  fi
  if [ "$sc_made" = 1 ]; then k delete storageclass "$storage_class" --ignore-not-found >/dev/null 2>&1 || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# dash (sh on the runners) runs the EXIT trap on `exit` and not when a signal ends the script: each signal becomes an exit, with the usual status.
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# jp <resource> <jsonpath>: a field of an object of the namespace, empty when it is not there.
jp() { k -n "$ns" get "$1" -o "jsonpath=$2" 2>/dev/null || true; }
# cond <resource> <type>: the status of a condition of an object of the namespace, empty when it is not there.
cond() {
  k -n "$ns" get "$1" -o json 2>/dev/null | jq -r --arg t "$2" '[.status.conditions[]? | select(.type == $t) | .status][0] // ""' || true
}
# until_ok <seconds> <description> <command...>: poll every 3 s; the output of the last attempt is what the failure shows.
until_ok() {
  _limit=$1; _what=$2; shift 2
  _deadline=$(( $(date +%s) + _limit ))
  while ! last=$("$@" 2>&1); do
    [ "$(date +%s)" -lt "$_deadline" ] || die "gave up after ${_limit}s waiting for: $_what. The last attempt said: $(printf '%s' "$last" | tail -c 900)"
    sleep 3
  done
}

step "1. namespaces, a storage class, the CloudNativePG operator"
make_namespace() {
  jq -n --arg n "$1" --arg k "$made_by_key" --arg v "$made_by" '{apiVersion: "v1", kind: "Namespace", metadata: {name: $n, labels: {($k): $v}}}' | k create -f - >/dev/null
  ns_made=1
}
make_namespace "$ns"
make_namespace "$ns_ops"
ok "namespaces $ns and $ns_ops made, with the label $made_by_key=$made_by"
# The chart's default storage class is `longhorn` (database.storage.storageClass, orchestrator.artifacts.storageClass). On kind it is the class `standard`'s provisioner under that name.
storage_class=$(sed -n 's/^ *storageClass: "\(.*\)"$/\1/p' "$work/cnpg-clusters.yaml" | head -n 1)
[ -n "$storage_class" ] || die "the render has no storage class"
if k get storageclass "$storage_class" >/dev/null 2>&1; then
  note "the cluster has a StorageClass $storage_class already: it is used, and it is not removed"
else
  provisioner=$(k get storageclass standard -o jsonpath='{.provisioner}' 2>/dev/null || true)
  [ -n "$provisioner" ] || die "the cluster has no StorageClass $storage_class and no StorageClass standard to copy the provisioner from"
  sc_made=1
  k get storageclass standard -o json | jq --arg n "$storage_class" --arg k "$made_by_key" --arg v "$made_by" \
    '{apiVersion: "storage.k8s.io/v1", kind: "StorageClass", metadata: {name: $n, labels: {($k): $v}}, provisioner: .provisioner, reclaimPolicy: (.reclaimPolicy // "Delete"), volumeBindingMode: (.volumeBindingMode // "WaitForFirstConsumer")}' \
    | k apply -f - >/dev/null
  ok "StorageClass $storage_class made with the provisioner $provisioner (the one of standard)"
fi
mkdir -p "$work/dl"
curl -fsSL --retry 3 --retry-delay 3 -o "$work/dl/cloudnative-pg.tgz" "$cnpg_chart_url" || die "cannot download $cnpg_chart_url"
[ "$(sha256_of "$work/dl/cloudnative-pg.tgz")" = "$cnpg_chart_sha256" ] || die "$cnpg_chart_url is not the file backup-e2e/UPSTREAM pins ($cnpg_chart_sha256)"
crds_made=1
cnpg_installed=1
if ! helm install cnpg "$work/dl/cloudnative-pg.tgz" --namespace "$ns_ops" --wait --timeout "$helm_timeout" > "$work/helm.log" 2>&1; then
  tail -n 30 "$work/helm.log" | redact
  die "helm install cnpg did not finish"
fi
ok "the CloudNativePG operator $(k -n "$ns_ops" get deployment cnpg-cloudnative-pg -o jsonpath='{.spec.template.spec.containers[0].image}') runs in $ns_ops"
k wait --for=condition=Established "crd/$r_cluster" "crd/$r_pooler" --timeout=60s >/dev/null || die "the CRDs Cluster and Pooler are not established"
ok "the CRDs Cluster and Pooler are established"

step "2. the chart's database objects: a Cluster of three instances and a Pooler"
k -n "$ns" apply -f "$work/cnpg-clusters.yaml" -f "$work/cnpg-pooler.yaml" >/dev/null
ok "the Cluster $db and the Pooler $pooler are applied (the operator's webhooks and the CRDs' schemas accepted them)"
cluster_ready() {
  _st=$(cond "$r_cluster/$db" Ready)
  _in=$(jp "$r_cluster/$db" '{.status.instances}')
  _rd=$(jp "$r_cluster/$db" '{.status.readyInstances}')
  _pr=$(jp "$r_cluster/$db" '{.status.currentPrimary}')
  printf 'Ready=%s instances=%s readyInstances=%s primary=%s\n' "${_st:-unset}" "${_in:-0}" "${_rd:-0}" "${_pr:-none}"
  [ "$_st" = True ] && [ "${_in:-0}" = 3 ] && [ "${_rd:-0}" = 3 ] && [ -n "$_pr" ]
}
until_ok "$cluster_timeout" "the cluster $db to be Ready with 3 instances" cluster_ready
primary=$(jp "$r_cluster/$db" '{.status.currentPrimary}')
ok "the cluster $db is Ready with 3 instances ($(jp "$r_cluster/$db" '{.status.image}'), primary $primary)"

psql_on() { # psql_on POD SQL: the result as plain rows (the last statement's), as the superuser by the socket
  k -n "$ns" exec "$1" -c postgres -- psql -X -q -At -v ON_ERROR_STOP=1 -d orchestrator -c "$2" </dev/null
}
# The pooler's pods carry the label cnpg.io/cluster too (pkg/specs/pgbouncer/deployments.go): the database's pods are the ones whose podRole is instance.
db_pods="cnpg.io/cluster=$db,cnpg.io/podRole=instance"
replicas() { # the pods of the cluster that are not the primary
  k -n "$ns" get pods -l "$db_pods" -o json | jq -r --arg p "$primary" '.items[] | select(.metadata.name != $p) | .metadata.name'
}
streaming() {
  _n=$(psql_on "$primary" "SELECT count(*) FROM pg_stat_replication WHERE state = 'streaming'")
  printf 'streaming replicas: %s\n' "$_n"
  [ "$_n" = 2 ]
}
until_ok 120 "the primary to stream to two replicas" streaming
[ "$(replicas | wc -l | tr -d ' ')" = 2 ] || die "the cluster does not have two pods besides the primary: $(replicas | tr '\n' ' ')"
ok "the primary $primary streams to two replicas ($(replicas | tr '\n' ' '))"
k -n "$ns" get pods -l "$db_pods" -o json | jq -e '
  [.items[] | .spec.containers[] | select(.name == "postgres") | .resources]
  | length == 3 and all(.requests.cpu == "500m" and .requests.memory == "1Gi" and .limits.memory == "2Gi")' >/dev/null \
  || die "the three PostgreSQL pods do not all have the resources the values ask for (requests 500m and 1Gi, memory limit 2Gi): $(k -n "$ns" get pods -l "$db_pods" -o json | jq -c '[.items[] | .spec.containers[] | select(.name == "postgres") | .resources]')"
ok "each of the three PostgreSQL pods has the resources of the values: requests 500m CPU and 1Gi, a memory limit of 2Gi"

pooler_ready() {
  _av=$(jp "deployment/$pooler" '{.status.availableReplicas}')
  _im=$(jp "$r_pooler/$pooler" '{.status.image}')
  printf 'availableReplicas=%s image=%s\n' "${_av:-0}" "${_im:-none}"
  [ "${_av:-0}" = 2 ] && [ -n "$_im" ]
}
until_ok "$pooler_timeout" "the Pooler's Deployment $pooler to have 2 available pods" pooler_ready
pooler_image=$(jp "$r_pooler/$pooler" '{.status.image}')
ok "the Pooler $pooler runs two pods of $pooler_image"
pgb_version=${pooler_image##*:}
case $pgb_version in
  [0-9]*.[0-9]*)
    _major=${pgb_version%%.*}; _rest=${pgb_version#*.}; _minor=${_rest%%.*}
    if [ "$_major" -gt 1 ] || { [ "$_major" -eq 1 ] && [ "$_minor" -ge 21 ]; }; then
      ok "PgBouncer $pgb_version is 1.21 or later, which max_prepared_statements needs"
    else
      die "PgBouncer $pgb_version is older than 1.21, which max_prepared_statements needs"
    fi
    ;;
  *) die "the pooler's image tag '$pgb_version' is not a version: the script cannot say it is 1.21 or later ($pooler_image)" ;;
esac
k -n "$ns" get service "$pooler" -o json | jq -e '.spec.ports | any(.port == 5432)' >/dev/null || die "the Service $pooler does not serve 5432"
k -n "$ns" get deployment "$pooler" -o json | jq -e '[.spec.template.spec.containers[] | select(.name == "pgbouncer") | .resources] | length == 1 and all(.requests.cpu == "100m" and .requests.memory == "128Mi" and .limits.memory == "256Mi")' >/dev/null \
  || die "the pooler's pgbouncer container does not have the resources the values ask for: $(k -n "$ns" get deployment "$pooler" -o json | jq -c '[.spec.template.spec.containers[] | .name, .resources]')"
ok "the Service $pooler serves 5432 and the pgbouncer container has the resources of the values (requests 100m and 128Mi, a memory limit of 256Mi)"
# PgBouncer's configuration is written by the pooler pod's instance manager to /controller/configs/pgbouncer.ini (pkg/management/pgbouncer/config at v1.30.1: ConfigsDir is
# the scratch directory /controller plus /configs; the password of the auth user is in userlist.txt, a file of its own, which is not read here). The path is read from the
# source, not run: when the file cannot be read, the script says so and the pool mode and the parsed statements are asserted from the pooler's metrics in step 6 instead.
pooler_pod=$(k -n "$ns" get pods -l "cnpg.io/poolerName=$pooler" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)
[ -n "$pooler_pod" ] || die "the Pooler has no pod"
if k -n "$ns" exec "$pooler_pod" -c pgbouncer -- cat /controller/configs/pgbouncer.ini > "$work/pgbouncer.ini" 2>/dev/null && [ -s "$work/pgbouncer.ini" ]; then
  grep -Eq '^pool_mode *= *transaction$' "$work/pgbouncer.ini" || die "PgBouncer's pool_mode is not transaction: $(grep -E '^pool_mode' "$work/pgbouncer.ini" | head -n 1)"
  for _l in '^max_prepared_statements *= *200$' '^max_client_conn *= *1000$' '^default_pool_size *= *20$'; do
    grep -Eq "$_l" "$work/pgbouncer.ini" || die "PgBouncer's configuration lacks the line $_l (the chart's value): $(grep -E '^(max_prepared_statements|max_client_conn|default_pool_size)' "$work/pgbouncer.ini" | tr '\n' ' ')"
  done
  ok "PgBouncer's pgbouncer.ini (in the pod $pooler_pod) says pool_mode = transaction, max_prepared_statements = 200, max_client_conn = 1000, default_pool_size = 20"
else
  note "PgBouncer's pgbouncer.ini could not be read at /controller/configs in the pod $pooler_pod: the pool mode and the parsed statements are asserted from the pooler's metrics below"
fi

step "3. the stand-ins: a mock issuer, a mock agent, a pod with curl, and the orchestrator's Secret"
k -n "$ns" create secret generic oidc-client --from-file=CLIENT_SECRET="$work/oidc-secret" >/dev/null
# What the chart's ExternalSecret would have made (ExternalSecrets are off in the values): the keys the Deployment reads. The agent's bearer is the key CODER_A2A_TOKEN.
k -n "$ns" create secret generic "$orch" --from-file=thread-tools-secret="$work/thread-tools-secret" --from-file=CODER_A2A_TOKEN="$work/agent-token" >/dev/null
k -n "$ns" label secret oidc-client "$orch" "$made_by_key=$made_by" >/dev/null
k -n "$ns" create configmap oidc-app --from-file=server.mjs="$repo/dev/mock-oidc/server.mjs" --from-file=users.json="$repo/dev/mock-oidc/users.json" >/dev/null
k -n "$ns" create configmap mock-agent-mappings --from-file="$repo/dev/wiremock/agent/mappings" >/dev/null
k -n "$ns" create configmap mock-agent-files --from-file="$repo/dev/wiremock/agent/__files" >/dev/null
cat > "$work/stand-ins.yaml" <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: oidc
spec:
  replicas: 1
  selector: {matchLabels: {app: oidc}}
  template:
    metadata:
      labels: {app: oidc}
    spec:
      containers:
        - name: oidc
          image: $node_image
          # server.mjs starts only when it is the main module by its own path (import.meta.url against argv[1]), and a ConfigMap volume is made of symlinks, which node
          # resolves: from there the file is a different path and the process ends with status 0 and says nothing. So both files are copied to a plain directory first.
          command: ["sh", "-c", "cp /src/server.mjs /src/users.json /work/ && exec node /work/server.mjs"]
          securityContext: {runAsUser: 1000, runAsNonRoot: true, allowPrivilegeEscalation: false}
          env:
            - {name: PORT, value: "8080"}
            # The issuer the orchestrator is configured with in this run (the script changes that one line of the chart's ConfigMap) and the client the chart's audience names.
            - {name: ISSUER, value: "http://oidc:8080"}
            - {name: CLIENT_ID, value: another-agentic}
            - name: CLIENT_SECRET
              valueFrom: {secretKeyRef: {name: oidc-client, key: CLIENT_SECRET}}
            - {name: ROLES_CLAIM, value: agentic_roles}
            - {name: WEB_CLIENT_ID, value: ""}
            - {name: MOCK_OIDC_USERS, value: /work/users.json}
          ports: [{containerPort: 8080}]
          readinessProbe: {httpGet: {path: /healthz, port: 8080}, periodSeconds: 3}
          volumeMounts:
            - {name: src, mountPath: /src, readOnly: true}
            - {name: work, mountPath: /work}
      volumes:
        - name: src
          configMap: {name: oidc-app}
        - name: work
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: oidc
spec:
  selector: {app: oidc}
  ports: [{name: http, port: 8080, targetPort: 8080}]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mock-agent
spec:
  replicas: 1
  selector: {matchLabels: {app: mock-agent}}
  template:
    metadata:
      labels: {app: mock-agent}
    spec:
      # The stubs are copied out of the ConfigMaps (a volume of symlinks, with the hidden directories Kubernetes keeps in it, which WireMock would read as a second set) into plain directories (the glob skips the hidden ones).
      initContainers:
        - name: stubs
          image: $wiremock_image
          command: ["sh", "-c", "cp -L /src/mappings/* /dst/mappings/ && cp -L /src/files/* /dst/files/"]
          volumeMounts:
            - {name: src-mappings, mountPath: /src/mappings, readOnly: true}
            - {name: src-files, mountPath: /src/files, readOnly: true}
            - {name: mappings, mountPath: /dst/mappings}
            - {name: files, mountPath: /dst/files}
      containers:
        - name: wiremock
          image: $wiremock_image
          # The arguments compose.yaml's x-wiremock gives every mock: the stubs fill their answers in from the request (the card's interface URL follows the Host header).
          args: ["--global-response-templating", "--disable-banner"]
          ports: [{containerPort: 8080}]
          readinessProbe: {httpGet: {path: /__admin/health, port: 8080}, periodSeconds: 3}
          volumeMounts:
            - {name: mappings, mountPath: /home/wiremock/mappings}
            - {name: files, mountPath: /home/wiremock/__files}
      volumes:
        - name: src-mappings
          configMap: {name: mock-agent-mappings}
        - name: src-files
          configMap: {name: mock-agent-files}
        - name: mappings
          emptyDir: {}
        - name: files
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: mock-agent
spec:
  selector: {app: mock-agent}
  ports: [{name: http, port: 8080, targetPort: 8080}]
---
apiVersion: v1
kind: Pod
metadata:
  name: curl-client
  labels: {app: curl-client}
spec:
  terminationGracePeriodSeconds: 0
  containers:
    - name: curl
      image: $curl_image
      command: ["sleep", "3600"]
EOF
k -n "$ns" apply -f "$work/stand-ins.yaml" >/dev/null
k -n "$ns" rollout status deployment/oidc --timeout=240s >/dev/null || die "the mock issuer did not become ready"
k -n "$ns" rollout status deployment/mock-agent --timeout=240s >/dev/null || die "the mock agent did not become ready"
k -n "$ns" wait --for=condition=Ready pod/curl-client --timeout=240s >/dev/null || die "the curl pod did not become ready"
ok "the mock issuer ($node_image), the mock agent ($wiremock_image) and a curl pod run"
c() { k -n "$ns" exec curl-client -- curl -sS --max-time 40 "$@"; }
card=$(c http://mock-agent:8080/.well-known/agent-card.json) || die "the mock agent's card does not answer"
printf '%s' "$card" | jq -e '.supportedInterfaces[0].url | test("^http://mock-agent:8080/")' >/dev/null || die "the mock agent's card does not name http://mock-agent:8080 as its interface: $(printf '%s' "$card" | head -c 300)"
ok "the mock agent's card names http://mock-agent:8080/a2a (the Host header the orchestrator uses)"
token_for() { # token_for EMAIL: an access token of the mock issuer for that user
  c -u "another-agentic:$oidc_secret" --data-urlencode grant_type=client_credentials --data-urlencode "user=$1" http://oidc:8080/token | jq -r '.access_token // empty'
}
token=$(token_for dev@example.com)
[ -n "$token" ] || die "the mock issuer gave no token for dev@example.com"
printf '%s\n' "$token" >> "$work/secrets"
printf '%s' "$token" | jq -R 'split(".") | .[1] | gsub("-"; "+") | gsub("_"; "/") | @base64d | fromjson' > "$work/claims.json" 2>/dev/null || die "the token is not a JWT"
jq -e '.iss == "http://oidc:8080" and .aud == "another-agentic" and (.agentic_roles | index("user")) and .email == "dev@example.com"' "$work/claims.json" >/dev/null \
  || die "the token's claims are not the chart's (aud another-agentic, the role in agentic_roles, the e-mail): $(jq -c '{iss, aud, agentic_roles, email}' "$work/claims.json")"
ok "the mock issuer gives dev@example.com a token for the audience another-agentic with the role user in agentic_roles"

step "4. the orchestrator, started with the chart's configuration, and a sampler of the database's sessions while it boots"
# The chart's ConfigMap, with two lines changed: the environment and the issuer. A production orchestrator takes only an https issuer, and a kind cluster has no
# certificate authority for one; nothing else of the configuration is touched, and the script says so by asserting it.
sed -e 's|^      environment: production$|      environment: development|' \
    -e 's|^        issuer: "https://auth.pooler-e2e.example.org/realms/e2e"$|        issuer: "http://oidc:8080"|' \
    "$work/orchestrator-configmap.yaml" > "$work/orchestrator-configmap.run.yaml"
changed=$(diff "$work/orchestrator-configmap.yaml" "$work/orchestrator-configmap.run.yaml" | grep -c '^[<>]' || true)
[ "$changed" = 4 ] || die "the run's ConfigMap differs from the chart's by $changed lines, not 4 (two lines replaced): $(diff "$work/orchestrator-configmap.yaml" "$work/orchestrator-configmap.run.yaml" | head -n 12)"
diff "$work/orchestrator-configmap.yaml" "$work/orchestrator-configmap.run.yaml" | grep -q 'environment: development' || die "the run's ConfigMap does not have environment: development"
diff "$work/orchestrator-configmap.yaml" "$work/orchestrator-configmap.run.yaml" | grep -q 'issuer: "http://oidc:8080"' || die "the run's ConfigMap does not have the mock issuer"
ok "the run's ConfigMap differs from the chart's in exactly two lines: server.environment (development) and auth.jwt.issuer (http://oidc:8080)"

# The sampler: on the primary, one psql that asks every 20 ms for the sessions of the database `orchestrator` that have a client address (the instance manager uses the
# socket, so every such session is a client's), with the first characters of what each last ran. It starts before the Deployment and is stopped when the orchestrator is
# ready. The migrations of an empty database take a fraction of a second: they are caught only if a sample falls in them, and step 6 says what it does when none did.
cat > "$work/sampler.sql" <<'EOF'
SELECT host(client_addr), state, replace(left(query, 80), chr(10), ' ') FROM pg_stat_activity WHERE datname = current_database() AND client_addr IS NOT NULL AND pid <> pg_backend_pid()
\watch 0.02
EOF
k -n "$ns" exec -i "$primary" -c postgres -- psql -X -q -At -F '|' -d orchestrator -f - < "$work/sampler.sql" > "$work/sampler.out" 2>/dev/null &
sampler_pid=$!
sleep 2
k -n "$ns" apply -f "$work/orchestrator-configmap.run.yaml" -f "$work/orchestrator-pvc.yaml" -f "$work/orchestrator-service.yaml" -f "$work/orchestrator-deployment.yaml" >/dev/null
ok "the orchestrator's ConfigMap, claim, Service and Deployment are applied (image $orch_image)"
if ! k -n "$ns" rollout status "deployment/$orch" --timeout="${orchestrator_timeout}s" >/dev/null 2>&1; then
  die "the orchestrator did not become ready within ${orchestrator_timeout}s (its readiness probe is /readyz: 503 until the database answers and the issuer's keys are fetched)"
fi
sleep 3
kill "$sampler_pid" >/dev/null 2>&1 || true
sampler_pid=''
ok "the Deployment $orch is ready: its /readyz answered 200 to the kubelet"
[ "$(c -o /dev/null -w '%{http_code}' "http://$orch:8080/healthz")" = 200 ] || die "/healthz is not 200"
[ "$(c -o /dev/null -w '%{http_code}' "http://$orch:8080/readyz")" = 200 ] || die "/readyz is not 200"
ok "/healthz and /readyz answer 200 from another pod"
k -n "$ns" logs "deployment/$orch" > "$work/orch.log" 2>&1 || die "no log of the orchestrator"
grep -q 'database.directUrl is set' "$work/orch.log" || die "the orchestrator did not log 'database.directUrl is set': it did not take the key (its log: $(head -c 600 "$work/orch.log"))"
grep -q 'database migrations applied' "$work/orch.log" || die "the orchestrator did not log 'database migrations applied'"
ok "the orchestrator logged that database.directUrl is set (the migrations and the LISTEN connection use it) and that its migrations were applied"
orch_ip=$(jp "pod/$(k -n "$ns" get pods -l app.kubernetes.io/component=orchestrator -o jsonpath='{.items[0].metadata.name}')" '{.status.podIP}')
pooler_ips=$(k -n "$ns" get pods -l "cnpg.io/poolerName=$pooler" -o json | jq -r '.items[].status.podIP')
[ -n "$orch_ip" ] || die "the orchestrator's pod has no address"
[ "$(printf '%s\n' "$pooler_ips" | grep -c .)" = 2 ] || die "the pooler does not have two pod addresses: '$pooler_ips'"
note "addresses: orchestrator $orch_ip, pooler $(printf '%s' "$pooler_ips" | tr '\n' ' ')"

step "5. a request through the stack: a thread written and read"
auth="Authorization: Bearer $token"
api() { # api METHOD PATH [curl args]: the body on stdout, non-zero when the status is not 2xx
  _m=$1; _p=$2; shift 2
  c --fail-with-body -X "$_m" "http://$orch:8080$_p" -H "$auth" "$@"
}
api GET /api/me | jq -e '.user == "dev@example.com" and (.roles | index("user"))' >/dev/null || die "GET /api/me is not dev@example.com with the role user: $(api GET /api/me | head -c 300)"
ok "GET /api/me: dev@example.com, role user (the token is the issuer's and the orchestrator verified it)"
thread=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f')
run=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f')
msg=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f')
text='pooler e2e: hello through PgBouncer'
jq -n --arg thread "$thread" --arg run "$run" --arg msg "$msg" --arg text "$text" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}' > "$work/run-input.json"
pool_metric() { # pool_metric NAME: the sum, over the pooler pods, of a metric of the database `orchestrator` (0 when no pod has the line)
  _sum=0
  for _pod in $(k -n "$ns" get pods -l "cnpg.io/poolerName=$pooler" -o name | sed 's|^pod/||'); do
    _v=$(k get --raw "/api/v1/namespaces/$ns/pods/$_pod:9127/proxy/metrics" 2>/dev/null | awk -v n="$1" '$0 ~ ("^" n "\\{") && $0 ~ /database="orchestrator"/ { s += $NF } END { printf "%d", s }')
    _sum=$(( _sum + ${_v:-0} ))
  done
  printf '%s' "$_sum"
}
xact_before=$(pool_metric cnpg_pgbouncer_stats_total_xact_count)
note "transactions the pooler has carried for the database orchestrator so far: $xact_before"
# The run is a stream: the response ends with the run (the mock agent completes at once). A run that does not end is cut by --max-time.
k -n "$ns" exec -i curl-client -- curl -sS -N --max-time 60 -X POST "http://$orch:8080/agui/agents/mock-coder" -H "$auth" \
  -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary @- < "$work/run-input.json" > "$work/run.sse" 2>"$work/run.err" || true
grep -q '"type":"RUN_FINISHED"' "$work/run.sse" || die "the run did not finish (no RUN_FINISHED in its stream): $(head -c 400 "$work/run.sse") $(head -c 200 "$work/run.err")"
ok "POST /agui/agents/mock-coder started thread $thread and its run finished"
thread_done() {
  _s=$(api GET "/api/threads/$thread" | jq -r '.state // empty')
  printf 'state=%s\n' "${_s:-unknown}"
  [ "$_s" = "done" ]
}
until_ok 90 "the thread $thread to end done" thread_done
ok "the thread ends done (the dispatcher claimed its job through the pooler and the mock agent completed it)"
title='pooler e2e: renamed'
api PATCH "/api/threads/$thread" -H 'content-type: application/json' -d "$(jq -n --arg t "$title" '{title: $t}')" | jq -e --arg t "$title" '.title == $t' >/dev/null || die "PATCH /api/threads/$thread did not return the new title"
[ "$(api GET "/api/threads/$thread" | jq -r '.title')" = "$title" ] || die "GET /api/threads/$thread does not say the new title"
api GET /api/threads | jq -e --arg id "$thread" 'any(.[]; .id == $id and .title == "pooler e2e: renamed")' >/dev/null || die "GET /api/threads does not list the thread with its new title"
ok "PATCH renamed the thread (a write), GET /api/threads/$thread and the list read the new title back"
api GET "/api/threads/$thread/export" | jq -e --arg t "$text" '[.. | strings] | any(. == $t)' >/dev/null || die "the thread's export does not hold the person's message"
ok "the thread's export holds the person's message"
[ "$(psql_on "$primary" "SELECT count(*) FROM threads WHERE id = '$thread' AND title = '$title'")" = 1 ] || die "the primary does not hold the thread $thread with its new title"
ok "the primary holds the thread row, with the new title"
replica_has() {
  _n=0
  for _r in $(replicas); do
    [ "$(psql_on "$_r" "SELECT count(*) FROM threads WHERE id = '$thread' AND title = '$title'")" = 1 ] && _n=$(( _n + 1 ))
  done
  printf 'replicas that hold the row: %s of 2\n' "$_n"
  [ "$_n" = 2 ]
}
until_ok 60 "both replicas to hold the thread row" replica_has
ok "both replicas hold the thread row too (three instances, streamed)"

step "6. who connected: the database's sessions, the sampled boot, and the pooler's counters"
sessions() { # the database's client sessions now: address|state|query
  psql_on "$primary" "SELECT host(client_addr) || '|' || state || '|' || replace(left(query, 80), chr(10), ' ') FROM pg_stat_activity WHERE datname = 'orchestrator' AND client_addr IS NOT NULL AND backend_type = 'client backend' AND pid <> pg_backend_pid() ORDER BY 1"
}
sessions > "$work/sessions.txt" || die "cannot read pg_stat_activity"
from_orch=$(awk -F'|' -v ip="$orch_ip" '$1 == ip' "$work/sessions.txt")
[ -n "$from_orch" ] || die "no session of the database comes from the orchestrator's own address $orch_ip: the wakeup's LISTEN connection should (sessions: $(tr '\n' ';' < "$work/sessions.txt"))"
not_listen=$(printf '%s\n' "$from_orch" | awk -F'|' '$3 !~ /^LISTEN /')
[ -z "$not_listen" ] || die "a session from the orchestrator's own address, which reaches the database itself, is not the LISTEN connection: $not_listen"
ok "every session that comes from the orchestrator's address ($(printf '%s\n' "$from_orch" | wc -l | tr -d ' ')) is the wakeup's LISTEN: direct, as database.directUrl says"
via_pooler=0
for ip in $pooler_ips; do
  n=$(awk -F'|' -v ip="$ip" '$1 == ip' "$work/sessions.txt" | wc -l | tr -d ' ')
  via_pooler=$(( via_pooler + n ))
  if awk -F'|' -v ip="$ip" '$1 == ip && $3 ~ /^LISTEN /' "$work/sessions.txt" | grep -q .; then die "a LISTEN session comes from the pooler $ip: a transaction pooler cannot keep it"; fi
done
[ "$via_pooler" -ge 1 ] || die "no session of the database comes from a pooler pod: the requests do not go through the pooler (sessions: $(tr '\n' ';' < "$work/sessions.txt"))"
ok "$via_pooler sessions of the database come from the two pooler pods, none of them a LISTEN: the requests, the dispatcher and the rest go through PgBouncer"
others=$(awk -F'|' -v o="$orch_ip" -v p1="$(printf '%s\n' "$pooler_ips" | sed -n 1p)" -v p2="$(printf '%s\n' "$pooler_ips" | sed -n 2p)" '$1 != o && $1 != p1 && $1 != p2' "$work/sessions.txt")
if [ -n "$others" ]; then note "sessions from other addresses (information only): $(printf '%s' "$others" | tr '\n' ';')"; fi

# The sampled boot: what the primary showed while the Deployment rolled out.
migration='_sqlx_migrations|pg_advisory_lock|pg_advisory_unlock|CREATE TABLE|CREATE INDEX|ALTER TABLE'
n_samples=$(wc -l < "$work/sampler.out" | tr -d ' ')
note "the sampler recorded $n_samples session rows while the orchestrator started"
for ip in $pooler_ips; do
  if awk -F'|' -v ip="$ip" '$1 == ip' "$work/sampler.out" | grep -Eq "$migration"; then
    die "the pooler pod $ip ran a migration statement (sqlx's lock, _sqlx_migrations or DDL): the migrations went through the pooler, not the database itself"
  fi
done
direct_boot=$(awk -F'|' -v ip="$orch_ip" '$1 == ip' "$work/sampler.out" | grep -Ev '\|LISTEN ' | grep -E "$migration" || true)
if [ -n "$direct_boot" ]; then
  ok "while the orchestrator started, a session from its OWN address ran the migrations ($(printf '%s\n' "$direct_boot" | head -n 1 | cut -d '|' -f 3 | cut -c 1-60)...); no pooler pod ever did"
else
  note "the sampler caught no migration statement from any address (a migration is a second long); the orchestrator's log line and the listener above are the evidence, and no pooler pod ran one"
fi

# PgBouncer counts in a period of its own (stats_period, 60 s by default) and the exporter reads what it has: the counters are polled until they have moved, and the
# orchestrator's background work (the dispatcher, the inbox) keeps moving them.
xact_rose() {
  _x=$(pool_metric cnpg_pgbouncer_stats_total_xact_count)
  printf 'transactions for the database orchestrator: before %s, now %s\n' "$xact_before" "$_x"
  [ "$_x" -gt "$xact_before" ]
}
until_ok 150 "the pooler's count of transactions for the database orchestrator to rise" xact_rose
xact_after=$(pool_metric cnpg_pgbouncer_stats_total_xact_count)
ok "the pooler carried the requests: its transactions for the database orchestrator rose from $xact_before to $xact_after"
parsed() {
  _p=$(pool_metric cnpg_pgbouncer_stats_total_client_parse_count)
  printf 'prepared statements parsed by clients of the database orchestrator: %s\n' "$_p"
  [ "$_p" -gt 0 ]
}
until_ok 150 "the pooler to count prepared statements parsed by the orchestrator's clients (cnpg_pgbouncer_stats_total_client_parse_count)" parsed
ok "the pooler counted $(pool_metric cnpg_pgbouncer_stats_total_client_parse_count) prepared statements parsed by the orchestrator's clients (max_prepared_statements at work)"
pool_modes() { # the pool mode of every pool of the database orchestrator, over the pooler pods
  _m=''
  for _pod in $(k -n "$ns" get pods -l "cnpg.io/poolerName=$pooler" -o name | sed 's|^pod/||'); do
    _m="$_m $(k get --raw "/api/v1/namespaces/$ns/pods/$_pod:9127/proxy/metrics" 2>/dev/null | awk '/^cnpg_pgbouncer_pools_pool_mode\{/ && /database="orchestrator"/ { print $NF }' | tr '\n' ' ')"
  done
  printf '%s' "$_m" | tr -s ' ' | sed 's/^ //; s/ $//'
}
modes=$(pool_modes)
[ -n "$modes" ] || die "the pooler reports no pool for the database orchestrator"
case " $modes " in *" 1 "* | *" 3 "*) die "a pool of the database orchestrator is not in transaction mode (cnpg_pgbouncer_pools_pool_mode: $modes; 1 session, 2 transaction, 3 statement)" ;; esac
ok "the pooler's pool mode for the database orchestrator is transaction (cnpg_pgbouncer_pools_pool_mode: $modes)"

step "7. the run's password and secrets are in no ConfigMap, no Deployment, no pod, no event and no log"
k -n "$ns" get secret "$db-app" -o json | jq -r '.data.password // empty' | base64 -d > "$work/app-password" 2>/dev/null || true
[ -s "$work/app-password" ] || die "the app Secret $db-app has no password"
[ "$(wc -c < "$work/app-password" | tr -d ' ')" = 64 ] || die "the app Secret's password is not 64 characters long"
grep -Eq '^[A-Za-z0-9]{64}$' "$work/app-password" || die "the app Secret's password is not 64 letters and digits: a URL built from it would need escaping (this is CNPG's alphabet, UPSTREAM)"
ok "the generated password is 64 letters and digits, which a URL carries as it is"
cat "$work/app-password" >> "$work/secrets"
printf '\n' >> "$work/secrets"
: > "$work/everything"
for pod in $(k -n "$ns" get pods -o name); do
  k -n "$ns" logs "$pod" --all-containers --tail=-1 >> "$work/everything" 2>/dev/null || true
done
k -n "$ns" get configmaps,deployments,pods,events,services,"$r_cluster","$r_pooler" -o yaml >> "$work/everything" 2>/dev/null || true
[ -s "$work/everything" ] || die "nothing was read for the credentials check"
if grep -qF -f "$work/secrets" "$work/everything"; then
  die "a credential of this run (the password, a token, a secret) is in a ConfigMap, a Deployment, a pod, an event or a log (the line is not printed)"
fi
ok "the password and the run's secrets are in none of the ConfigMaps, Deployments, pods, events and logs ($(wc -l < "$work/everything" | tr -d ' ') lines read)"
if k -n "$ns" get deployment "$orch" -o json | jq -e '[.spec.template.spec.containers[].env[]? | select(.name | test("^ORCH_DB_(POOLED|DIRECT)_URL$")) | .value | contains("$(ORCH_DB_PASSWORD)")] | length == 2 and all' >/dev/null; then
  ok "both URLs in the Deployment are references to the password variable, which comes from the app Secret"
else
  die "the Deployment's two URL variables do not both name \$(ORCH_DB_PASSWORD)"
fi

printf '\nPOOLER E2E OK: three instances and a transaction-mode pooler ran, the orchestrator migrated and listened on the database itself and carried its requests through PgBouncer\n'
