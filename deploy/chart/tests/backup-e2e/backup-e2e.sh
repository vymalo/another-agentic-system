#!/bin/sh
# The kind end-to-end of the chart's database backups (issue #151, deploy/chart/README.md "Backups"): does what the chart renders back the database up to
# S3, and does examples/restore-cluster.yaml, applied as written, bring the data back? Everything is real but the model of nothing: CloudNativePG, the
# barman-cloud plugin, cert-manager and a RustFS, on a kind cluster. The chart's own file is applied (`helm template -s templates/cnpg-clusters.yaml -s
# templates/cnpg-backup.yaml`), with the values of tests/backup-e2e/values.yaml over examples/netcup.values.yaml.
#
#   sh deploy/chart/tests/backup-e2e/backup-e2e.sh           (from the repository root, kubectl pointed at a kind cluster)
#   sh deploy/chart/tests/backup-e2e/backup-e2e.sh render    (helm only: prints the objects the script would apply, and checks them)
#
# What it does, in order, and asserts as it goes:
#   1. the three charts of UPSTREAM (cert-manager, the CloudNativePG operator, the barman-cloud plugin) downloaded and compared with their pinned checksums,
#      installed: cert-manager and the operator each in a namespace of their own, the plugin in the operator's (it must be there);
#   2. a RustFS (rustfs.yaml) with credentials made for the run and the bucket the ObjectStore names; the same credentials in the Secret
#      `another-agentic-db-backup`, which is what the chart's ExternalSecret would have made (ExternalSecrets are off);
#   3. the chart's database objects applied: the Cluster (with the plugin entry), the ObjectStore and the ScheduledBackup (a cron of every minute, `immediate`
#      as the chart has it); the Cluster is Ready;
#   4. the schema: the migrations of orchestrator/crates/store-postgres, in order, with psql as the owner `orchestrator` (SET ROLE), and one thread, with a title
#      that is known; a WAL segment is switched, so that one is archived;
#   5. the backups: the Cluster's condition `ContinuousArchiving` is True; a Backup the ScheduledBackup made AFTER that thread was written is `completed`, made
#      by the plugin, and the Cluster's `LastBackupSucceeded` is True and the ObjectStore's `serverRecoveryWindow` says it; the bucket holds the base backup
#      (`<prefix>/another-agentic-db/base/<backup id>/backup.info`) and WAL (`.../wals/`);
#   6. the schedule is suspended (the newest base backup is then the one restored), a second thread is written AFTER that backup and its WAL segment is switched
#      and waited for in `pg_stat_archiver` and in the bucket: only the WAL has it;
#   7. the restore: examples/restore-cluster.yaml applied as it is written (its storage class is `longhorn`, the chart's default: the script gives the kind
#      cluster a class of that name, the same provisioner as `standard`), the new cluster is Ready, and `SELECT title FROM threads` as the owner returns BOTH
#      titles (the first from the base backup, the second from the WAL), with the tables of the source;
#   8. the restore archived nothing: the bucket still holds one server folder, after a WAL switch of the restored cluster and a wait;
#   9. none of the run's credentials is in a log or in an object of the namespace.
#
# What it does NOT prove: the netcup cluster (its CloudNativePG version, its Argo CD, its storage class, the pod rolling when the Cluster gains the plugin, the AWS
# S3 bucket and its IAM user), the ExternalSecret that makes the credentials Secret, the retention (28 days: the sweep runs every 30 minutes and the window is
# 28 days; nothing here ages a backup), the cutover of `examples/restore-cutover.yaml`, a restore to a point in time, or a database with the chat agent's and the
# coders' roles. deploy/chart/README.md "Backups" says what each of those is.
#
# Safety. The script installs and uninstalls Helm releases, applies CRDs, makes a StorageClass and deletes namespaces, so it runs only on a throwaway cluster
# and touches only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); BACKUP_E2E_ALLOW_ANY_CONTEXT=1 says another cluster is a throwaway one too. Checked
#     before anything is made and before the cleanup is armed.
#   * it refuses a cluster that has CloudNativePG, the plugin or cert-manager already (their CRDs exist): it would otherwise install over somebody's operator and
#     delete its CRDs afterwards. Use a fresh cluster. It refuses a namespace of its names that exists.
#   * the namespaces are BACKUP_E2E_NAMESPACE (default backup-e2e, the database and the RustFS), <it>-operators (CloudNativePG and the plugin) and
#     <it>-cert-manager, made with the label another-agentic-e2e/made-by=backup-e2e. The chart's objects and the RustFS are applied by their own names; the Secrets
#     are made with random values and printed nowhere.
#   * the cleanup undoes what the run got as far as making, each step recorded when it begins: the clusters and the schedule (with the operator still there), the
#     three Helm releases, the namespaces, the CRDs those releases installed (none was there before: the run refused otherwise) and the StorageClass, if the run
#     made it. A run that stops before any of that deletes and uninstalls nothing.
#
# Needs kubectl, helm (3.19), jq, curl, openssl, tar and sha256sum (or shasum); a cluster that reaches docker.io, ghcr.io, quay.io and github.com. Every wait is
# bounded; on a failure the cluster's state is printed (no line that holds one of the run's credentials). Environment (all optional): BACKUP_E2E_NAMESPACE,
# BACKUP_E2E_ALLOW_ANY_CONTEXT, HELM_TIMEOUT (default 8m), CLUSTER_TIMEOUT_SECS, BACKUP_TIMEOUT_SECS, WAL_TIMEOUT_SECS, RESTORE_TIMEOUT_SECS.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
chart=$repo/deploy/chart
upstream=$here/UPSTREAM
migrations=$repo/orchestrator/crates/store-postgres/migrations

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

# What the chart makes, by name: a release called `another-agentic-system` is what examples/restore-cluster.yaml assumes.
release=another-agentic-system
db=another-agentic-db
db_restore=another-agentic-db-restore
db_backup_secret=another-agentic-db-backup
r_cluster=clusters.postgresql.cnpg.io
r_backup=backups.postgresql.cnpg.io
r_sched=scheduledbackups.postgresql.cnpg.io
r_store=objectstores.barmancloud.cnpg.io

ns=${BACKUP_E2E_NAMESPACE:-backup-e2e}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "BACKUP_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac
[ "${#ns}" -le 40 ] || die "BACKUP_E2E_NAMESPACE='$ns' is longer than 40 characters"
ns_ops=$ns-operators
ns_cm=$ns-cert-manager

# render_db_objects: the chart's two files that make the database objects, with the values of this directory. The endpoint names the namespace.
render_db_objects() {
  helm template "$release" "$chart" --namespace "$ns" \
    -f "$chart/examples/netcup.values.yaml" -f "$here/values.yaml" \
    --set "backup.s3.endpoint=http://rustfs.$ns.svc:9000" \
    --show-only templates/cnpg-clusters.yaml --show-only templates/cnpg-backup.yaml
}

# check_render FILE: what the rest of the script depends on is in the render. Sets bucket, prefix, region and storage_class.
check_render() {
  [ "$(grep -c '^kind: ' "$1")" = 3 ] || die "the render is not exactly three objects (Cluster, ObjectStore, ScheduledBackup): $(grep '^kind: ' "$1" | tr '\n' ' ')"
  for _k in Cluster ObjectStore ScheduledBackup; do
    grep -q "^kind: $_k\$" "$1" || die "the render has no $_k"
  done
  grep -q "^  name: $db\$" "$1" || die "the render does not name the cluster $db (the release must be $release)"
  [ "$(grep -c "^        name: $db_backup_secret\$" "$1")" = 2 ] || die "the ObjectStore does not name the Secret $db_backup_secret twice (its two keys)"
  grep -q '^        key: ACCESS_KEY_ID$' "$1" || die "the ObjectStore does not read the key ACCESS_KEY_ID"
  grep -q '^        key: ACCESS_SECRET_KEY$' "$1" || die "the ObjectStore does not read the key ACCESS_SECRET_KEY"
  grep -q '^        barmanObjectName: '"$db"'$' "$1" || die "the Cluster's plugin entry does not name the ObjectStore $db"
  _dest=$(sed -n 's/^ *destinationPath: "s3:\/\/\(.*\)"$/\1/p' "$1")
  case $_dest in
    */*) bucket=${_dest%%/*}; prefix=${_dest#*/} ;;
    *) die "the ObjectStore's destinationPath has no bucket and prefix (the test wants both): '$_dest'" ;;
  esac
  region=$(sed -n '/name: AWS_DEFAULT_REGION/{n;s/^ *value: "\(.*\)"$/\1/p;}' "$1")
  storage_class=$(sed -n 's/^ *storageClass: "\(.*\)"$/\1/p' "$1")
  storage_size=$(sed -n 's/^ *size: "\(.*\)"$/\1/p' "$1")
  if [ -z "$bucket" ] || [ -z "$prefix" ] || [ -z "$region" ] || [ -z "$storage_class" ] || [ -z "$storage_size" ]; then
    die "the render lacks a bucket, prefix, region or storage"
  fi
  # examples/restore-cluster.yaml is applied as written, and its README says its storage is the deployment's: here, the chart's default.
  _ex=$chart/examples/restore-cluster.yaml
  grep -q "^    size: $storage_size\$" "$_ex" || die "examples/restore-cluster.yaml does not have the storage size of the render ($storage_size): it would not be applied as written"
  grep -q "^    storageClass: $storage_class\$" "$_ex" || die "examples/restore-cluster.yaml does not have the storage class of the render ($storage_class): it would not be applied as written"
  grep -q "^  name: $db_restore\$" "$_ex" || die "examples/restore-cluster.yaml is not the cluster $db_restore"
  if grep -q '^  plugins:' "$_ex"; then die "examples/restore-cluster.yaml has a plugins entry: a restored cluster must not archive"; fi
}

mode=${1:-run}
case $mode in
  run | render) ;;
  *) die "usage: backup-e2e.sh [render]" ;;
esac

if [ "$mode" = render ]; then
  command -v helm >/dev/null 2>&1 || die "helm is not installed"
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  render_db_objects > "$work/db-objects.yaml"
  check_render "$work/db-objects.yaml"
  cat "$work/db-objects.yaml"
  ok "the render is the Cluster, the ObjectStore and the ScheduledBackup; bucket $bucket, prefix $prefix, region $region, storage $storage_size $storage_class" >&2
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
    [ "${BACKUP_E2E_ALLOW_ANY_CONTEXT:-}" = 1 ] \
      || die "kubectl's current context is '$context', not a kind cluster's (kind-<name>): this script installs Helm releases, applies CRDs and deletes namespaces, so it will not run against it. BACKUP_E2E_ALLOW_ANY_CONTEXT=1 says that this cluster is a throwaway one"
    printf '::warning::%s\n' "kubectl's context '$context' is not a kind cluster's: BACKUP_E2E_ALLOW_ANY_CONTEXT=1 says it is a throwaway one"
    ;;
esac
# helm resolves its context by itself: the one that was just checked is the one it uses.
export HELM_KUBECONTEXT="$context"
k() { kubectl --context "$context" "$@"; }

made_by_key='another-agentic-e2e/made-by'
made_by='backup-e2e'
crd_pattern='\.(postgresql\.cnpg\.io|barmancloud\.cnpg\.io|cert-manager\.io)$'

k get --raw=/readyz >/dev/null 2>&1 || die "the cluster of the context '$context' does not answer"
existing=$(k get crd -o name 2>/dev/null | grep -E "$crd_pattern" || true)
if [ -n "$existing" ]; then
  die "the cluster has CRDs of CloudNativePG, the barman-cloud plugin or cert-manager already ($(printf '%s' "$existing" | head -n 3 | tr '\n' ' ')...): this script installs those itself and deletes their CRDs when it ends, so it needs a cluster that has none (kind delete cluster, kind create cluster)"
fi
for n in "$ns" "$ns_ops" "$ns_cm"; do
  if k get namespace "$n" >/dev/null 2>&1; then
    die "the namespace $n exists: a leftover of an earlier run (kubectl delete namespace $n, if it has the label $made_by_key=$made_by) or somebody's"
  fi
done

cnpg_chart_sha256=$(pin cnpg_chart_sha256)
plugin_chart_sha256=$(pin plugin_chart_sha256)
cm_chart_sha256=$(pin cert_manager_chart_sha256)
rustfs_image=$(pin rustfs_image)
grep -q "image: &rustfs-image $rustfs_image\$" "$repo/compose.yaml" \
  || die "UPSTREAM's rustfs_image is not the one compose.yaml's service rustfs runs ($rustfs_image): move them together"

helm_timeout=${HELM_TIMEOUT:-8m}
cluster_timeout=${CLUSTER_TIMEOUT_SECS:-480}
backup_timeout=${BACKUP_TIMEOUT_SECS:-420}
wal_timeout=${WAL_TIMEOUT_SECS:-180}
restore_timeout=${RESTORE_TIMEOUT_SECS:-600}

work=$(mktemp -d)
umask 077
title_a='backup-e2e: before the base backup'
title_b='backup-e2e: after the base backup, in the WAL only'
id_a=00000000-0000-4000-8000-00000000e2e1
id_b=00000000-0000-4000-8000-00000000e2e2
# The credentials of the run: RustFS's root credentials, which are the sidecar's too. They are in Secrets and must be found nowhere else.
access_key="bkacc$(openssl rand -hex 8)"
secret_key=$(openssl rand -hex 24)
printf '%s' "$access_key" > "$work/access-key"
printf '%s' "$secret_key" > "$work/secret-key"
printf '%s\n%s\n' "$access_key" "$secret_key" > "$work/secrets"

# What this run has made, recorded when it begins: the cleanup undoes these and nothing else.
ns_made=0       # the namespaces (the first of them made: the database's)
crds_made=0     # the first Helm install was started: the CRDs of the three charts may exist (none did before)
cm_installed=0
cnpg_installed=0
plugin_installed=0
sc_made=0
bucket=''; prefix=''; region=''; storage_class=''; storage_size=''

# redact: stdin to stdout without any line that holds one of the run's credentials, for the diagnostics.
redact() {
  if [ -s "$work/secrets" ]; then grep -vF -f "$work/secrets" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  k -n "$ns" get "$r_cluster,$r_store,$r_sched,$r_backup,pods,jobs,pvc" -o wide 2>&1 | redact || true
  k -n "$ns" get "$r_cluster/$db" -o yaml 2>&1 | redact | tail -n 130 || true
  k -n "$ns" get "$r_store/$db" -o yaml 2>&1 | redact | tail -n 60 || true
  k -n "$ns" get "$r_cluster/$db_restore" -o yaml 2>&1 | redact | tail -n 100 || true
  k -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 60 || true
  echo "::endgroup::"
  echo "::group::the database pods (Postgres and the plugin's sidecar) and the restore's job"
  k -n "$ns" logs -l "cnpg.io/cluster=$db" --all-containers --prefix --tail=120 2>&1 | redact || true
  k -n "$ns" logs -l "cnpg.io/cluster=$db_restore" --all-containers --prefix --tail=120 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the operator and the plugin"
  k -n "$ns_ops" get pods,deployments,services,certificates -o wide 2>&1 | redact || true
  k -n "$ns_ops" logs deployment/cnpg-cloudnative-pg --tail=150 2>&1 | redact || true
  k -n "$ns_ops" logs deployment/plugin-barman-cloud --tail=150 2>&1 | redact || true
  k -n "$ns_ops" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 30 || true
  echo "::endgroup::"
  echo "::group::cert-manager"
  k -n "$ns_cm" get pods -o wide 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::RustFS and its bucket"
  k -n "$ns" logs deployment/rustfs --tail=60 2>&1 | redact || true
  k -n "$ns" logs job/rustfs-bucket --tail=20 2>&1 | redact || true
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  if [ "$ns_made" = 1 ]; then
    # While the operator is still there: the schedule (its Backups go with it), then the clusters and the store.
    k -n "$ns" delete "$r_sched" --all --ignore-not-found --wait=true --timeout=60s >/dev/null 2>&1 || true
    k -n "$ns" delete "$r_cluster" --all --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
    k -n "$ns" delete "$r_backup" --all --ignore-not-found --wait=true --timeout=60s >/dev/null 2>&1 || true
    k -n "$ns" delete "$r_store" --all --ignore-not-found --wait=true --timeout=60s >/dev/null 2>&1 || true
  fi
  if [ "$plugin_installed" = 1 ]; then helm uninstall plugin-barman-cloud --namespace "$ns_ops" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$cnpg_installed" = 1 ]; then helm uninstall cnpg --namespace "$ns_ops" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$cm_installed" = 1 ]; then helm uninstall cert-manager --namespace "$ns_cm" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$ns_made" = 1 ]; then
    for n in "$ns" "$ns_ops" "$ns_cm"; do
      if k get namespace "$n" -o json 2>/dev/null | jq -e --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels[$k] == $v' >/dev/null 2>&1; then
        k delete namespace "$n" --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
      fi
    done
  fi
  if [ "$crds_made" = 1 ]; then
    # The charts keep their CRDs when they are uninstalled; none was in the cluster before this run (the run refused otherwise), so they are this run's.
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

step "0. the charts, with the checksums of UPSTREAM"
mkdir -p "$work/dl"
dl=$work/dl
fetch() { # fetch URLKEY FILE SHA256
  _url=$(pin "$1")
  curl -fsSL --retry 3 --retry-delay 3 -o "$dl/$2" "$_url" || die "cannot download $_url"
  _got=$(sha256_of "$dl/$2")
  [ "$_got" = "$3" ] || die "$_url has SHA-256 $_got, $upstream pins $3 ($1): the file moved or the pin is wrong"
  ok "$2 is $3"
}
fetch cert_manager_chart_url cert-manager.tgz "$cm_chart_sha256"
fetch cnpg_chart_url cloudnative-pg.tgz "$cnpg_chart_sha256"
fetch plugin_chart_url plugin-barman-cloud.tgz "$plugin_chart_sha256"
render_db_objects > "$work/db-objects.yaml"
check_render "$work/db-objects.yaml"
ok "the chart's render is the Cluster, the ObjectStore and the ScheduledBackup (bucket $bucket, prefix $prefix, region $region, storage $storage_size $storage_class)"
sed -e "s|__RUSTFS_IMAGE__|$rustfs_image|g" -e "s|__BUCKET__|$bucket|g" -e "s|__REGION__|$region|g" "$here/rustfs.yaml" > "$work/rustfs.yaml"

step "1. namespaces, a storage class, cert-manager, the CloudNativePG operator and the barman-cloud plugin"
make_namespace() {
  jq -n --arg n "$1" --arg k "$made_by_key" --arg v "$made_by" '{apiVersion: "v1", kind: "Namespace", metadata: {name: $n, labels: {($k): $v}}}' | k create -f - >/dev/null
  ns_made=1
}
make_namespace "$ns"
make_namespace "$ns_ops"
make_namespace "$ns_cm"
ok "namespaces $ns, $ns_ops and $ns_cm made, with the label $made_by_key=$made_by"
# examples/restore-cluster.yaml names the class of netcup (the chart's default). On kind it is the class `standard`'s provisioner under that name.
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

helm_install() { # helm_install RELEASE CHART NAMESPACE [args...]
  _rel=$1; _chart=$2; _ns=$3; shift 3
  if ! helm install "$_rel" "$_chart" --namespace "$_ns" "$@" --wait --timeout "$helm_timeout" > "$work/helm.log" 2>&1; then
    tail -n 30 "$work/helm.log" | redact
    die "helm install $_rel did not finish"
  fi
}
crds_made=1
cm_installed=1
helm_install cert-manager "$dl/cert-manager.tgz" "$ns_cm" --set crds.enabled=true
ok "cert-manager $(pin cert_manager_version) is installed in $ns_cm (its webhook answered the chart's startup check)"
cnpg_installed=1
helm_install cnpg "$dl/cloudnative-pg.tgz" "$ns_ops"
ok "the CloudNativePG operator $(k -n "$ns_ops" get deployment cnpg-cloudnative-pg -o jsonpath='{.spec.template.spec.containers[0].image}') runs in $ns_ops"
plugin_installed=1
helm_install plugin-barman-cloud "$dl/plugin-barman-cloud.tgz" "$ns_ops"
ok "the barman-cloud plugin $(k -n "$ns_ops" get deployment plugin-barman-cloud -o jsonpath='{.spec.template.spec.containers[0].image}') runs in $ns_ops"
k wait --for=condition=Established "crd/$r_cluster" "crd/$r_backup" "crd/$r_sched" "crd/$r_store" --timeout=60s >/dev/null \
  || die "the CRDs of the operator and the plugin are not established"
ok "the CRDs Cluster, Backup, ScheduledBackup and ObjectStore are established"

step "2. RustFS, its bucket $bucket and the credentials"
k -n "$ns" create secret generic rustfs-credentials --from-file=RUSTFS_ACCESS_KEY="$work/access-key" --from-file=RUSTFS_SECRET_KEY="$work/secret-key" >/dev/null
# What the chart's ExternalSecret would have made (templates/cnpg-backup.yaml, with externalSecrets.enabled=false it is the deployment's own).
k -n "$ns" create secret generic "$db_backup_secret" --from-file=ACCESS_KEY_ID="$work/access-key" --from-file=ACCESS_SECRET_KEY="$work/secret-key" >/dev/null
k -n "$ns" label secret rustfs-credentials "$db_backup_secret" "$made_by_key=$made_by" >/dev/null
k -n "$ns" apply -f "$work/rustfs.yaml" >/dev/null
k -n "$ns" rollout status deployment/rustfs --timeout=240s >/dev/null || die "RustFS did not become ready"
job_done() {
  _succeeded=$(jp job/rustfs-bucket '{.status.succeeded}')
  _failed=$(jp job/rustfs-bucket '{.status.failed}')
  printf 'succeeded=%s failed=%s\n' "${_succeeded:-0}" "${_failed:-0}"
  [ "${_succeeded:-0}" = 1 ]
}
until_ok 240 "the Job rustfs-bucket to make the bucket" job_done
ok "RustFS runs ($rustfs_image) and the bucket $bucket is there"

# --- the S3 helpers: ListObjectsV2 from inside the RustFS pod, with the pod's own credentials (SigV4 with the image's curl, as the bucket Job does; at most 1000 keys) ---
s3_list() { # s3_list PREFIX [DELIMITER]: the XML of the listing; the script in single quotes is the pod's own, its $1.. are the arguments after `sh`
  if [ -n "${2:-}" ]; then
    # shellcheck disable=SC2016
    k -n "$ns" exec deploy/rustfs -c rustfs -- sh -c '
      printf "user = \"%s:%s\"\n" "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" |
        curl -fsS -K - --aws-sigv4 "aws:amz:$1:s3" -G "http://127.0.0.1:9000/$2" --data-urlencode "list-type=2" --data-urlencode "prefix=$3" --data-urlencode "delimiter=$4"
    ' sh "$region" "$bucket" "$1" "$2" </dev/null
  else
    # shellcheck disable=SC2016
    k -n "$ns" exec deploy/rustfs -c rustfs -- sh -c '
      printf "user = \"%s:%s\"\n" "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" |
        curl -fsS -K - --aws-sigv4 "aws:amz:$1:s3" -G "http://127.0.0.1:9000/$2" --data-urlencode "list-type=2" --data-urlencode "prefix=$3"
    ' sh "$region" "$bucket" "$1" </dev/null
  fi
}
s3_keys() { # s3_keys PREFIX: the keys under it, one per line
  _xml=$(s3_list "$1") || return 1
  printf '%s' "$_xml" | grep -o '<Key>[^<]*</Key>' | sed -e 's/^<Key>//' -e 's/<\/Key>$//' || true
}
s3_folders() { # s3_folders PREFIX: the folders directly under it (with the trailing slash), one per line
  _xml=$(s3_list "$1" /) || return 1
  printf '%s' "$_xml" | grep -o '<CommonPrefixes><Prefix>[^<]*</Prefix>' | sed -e 's/^<CommonPrefixes><Prefix>//' -e 's/<\/Prefix>$//' || true
}

step "3. the chart's database objects, applied"
k -n "$ns" apply -f "$work/db-objects.yaml" >/dev/null
ok "the Cluster $db, the ObjectStore $db and the ScheduledBackup $db are applied (the operator's webhook and the CRDs' schemas accepted them)"
cluster_ready() { # cluster_ready NAME
  _st=$(cond "$r_cluster/$1" Ready)
  _ph=$(jp "$r_cluster/$1" '{.status.phase}')
  _pr=$(jp "$r_cluster/$1" '{.status.currentPrimary}')
  printf 'Ready=%s phase=%s primary=%s\n' "${_st:-unset}" "${_ph:-unset}" "${_pr:-none}"
  [ "$_st" = True ] && [ -n "$_pr" ]
}
until_ok "$cluster_timeout" "the cluster $db to be Ready" cluster_ready "$db"
ok "the cluster $db is Ready ($(jp "$r_cluster/$db" '{.status.image}'), primary $(jp "$r_cluster/$db" '{.status.currentPrimary}'))"
[ "$(jp "$r_cluster/$db" '{.spec.plugins[0].name}')" = barman-cloud.cloudnative-pg.io ] || die "the Cluster has no plugin entry for barman-cloud.cloudnative-pg.io"
# The plugin's sidecar is a native one: an init container whose restartPolicy is Always (plugin v0.15.1, internal/cnpgi/operator/lifecycle.go).
pod_containers() { # pod_containers POD: the names of its init containers and containers
  k -n "$ns" get pod "$1" -o json | jq -r '[(.spec.initContainers // [])[].name, .spec.containers[].name] | join(" ")'
}
containers=$(pod_containers "$(jp "$r_cluster/$db" '{.status.currentPrimary}')")
case " $containers " in *" plugin-barman-cloud "*) ok "the database pod has the plugin's sidecar (containers: $containers)" ;; *) die "the database pod has no plugin-barman-cloud sidecar (containers: $containers)" ;; esac

# --- the psql helpers: psql in the primary's postgres container, as the superuser by the socket (what `kubectl exec ... -- psql` is in the README) ---
primary_pod() { jp "$r_cluster/$1" '{.status.currentPrimary}'; }
psql_c() { # psql_c CLUSTER SQL: the result as plain rows (the last statement's)
  _pod=$(primary_pod "$1")
  [ -n "$_pod" ] || die "the cluster $1 has no primary pod"
  k -n "$ns" exec "$_pod" -c postgres -- psql -X -q -At -v ON_ERROR_STOP=1 -d orchestrator -c "$2" </dev/null
}
psql_stdin() { # psql_stdin CLUSTER: SQL on the standard input
  _pod=$(primary_pod "$1")
  [ -n "$_pod" ] || die "the cluster $1 has no primary pod"
  k -n "$ns" exec -i "$_pod" -c postgres -- psql -X -q -v ON_ERROR_STOP=1 -d orchestrator -f -
}
insert_thread() { # insert_thread ID RANK TITLE: as the owner
  printf "SET ROLE orchestrator;\nINSERT INTO threads (id, owner, title, agent_id, state, version, created_at, updated_at, rail_rank) VALUES ('%s', 'backup-e2e', '%s', 'chat', 'done', 1, now(), now(), '%s');\n" "$1" "$3" "$2" | psql_stdin "$db"
}

step "4. the schema and a thread"
count=0
for f in "$migrations"/*.sql; do
  [ -f "$f" ] || die "no migration in $migrations"
  { printf 'SET ROLE orchestrator;\n'; cat "$f"; } | psql_stdin "$db" || die "migration $(basename "$f") failed"
  count=$((count + 1))
done
tables_source=$(psql_c "$db" "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
ok "$count migrations applied in order as the owner orchestrator ($tables_source tables; $(psql_c "$db" 'SHOW server_version'))"
insert_thread "$id_a" 000000000001h "$title_a"
ok "a thread written: '$title_a'"
# The base backup that proves the first thread must START after it: the Backups that exist now do not count.
k -n "$ns" get "$r_backup" -l "cnpg.io/scheduled-backup=$db" -o json | jq -r '.items[].metadata.name' > "$work/known-backups"
switched=$(psql_c "$db" 'SELECT pg_walfile_name(pg_switch_wal() - 1)')
ok "the WAL segment $switched is switched, so that it is archived"

step "5. archiving, a base backup, the bucket"
archiving() {
  _st=$(cond "$r_cluster/$db" ContinuousArchiving)
  printf 'ContinuousArchiving=%s\n' "${_st:-unset}"
  [ "$_st" = True ]
}
until_ok "$wal_timeout" "the Cluster condition ContinuousArchiving to be True" archiving
ok "the Cluster's ContinuousArchiving is True (set by the instance manager after an archive through the plugin)"
new_backup() { # a completed Backup of the schedule that was not there when the first thread was written
  k -n "$ns" get "$r_backup" -l "cnpg.io/scheduled-backup=$db" -o json | jq -r '.items[] | "\(.metadata.name) \(.status.phase // "no phase") \(.status.error // "")"'
  k -n "$ns" get "$r_backup" -l "cnpg.io/scheduled-backup=$db" -o json | jq -r '.items[] | select(.status.phase == "completed") | .metadata.name' \
    | grep -vxF -f "$work/known-backups" | head -n 1 > "$work/new-backup" || true
  [ -s "$work/new-backup" ]
}
until_ok "$backup_timeout" "a Backup of the ScheduledBackup $db, made after the first thread, to be completed" new_backup
backup=$(cat "$work/new-backup")
k -n "$ns" get "$r_backup/$backup" -o json > "$work/backup.json"
[ "$(jq -r '.spec.method' "$work/backup.json")" = plugin ] || die "the Backup $backup was not made by the plugin: method $(jq -r '.spec.method' "$work/backup.json")"
[ "$(jq -r '.spec.cluster.name' "$work/backup.json")" = "$db" ] || die "the Backup $backup is not of the cluster $db"
backup_id=$(jq -r '.status.backupId // ""' "$work/backup.json")
[ -n "$backup_id" ] || die "the Backup $backup has no backupId"
ok "the Backup $backup (method plugin, backup id $backup_id) is completed; it started after the first thread was written ($(jq -r '.status.startedAt' "$work/backup.json"))"
last_succeeded() {
  _st=$(cond "$r_cluster/$db" LastBackupSucceeded)
  printf 'LastBackupSucceeded=%s\n' "${_st:-unset}"
  [ "$_st" = True ]
}
until_ok 60 "the Cluster condition LastBackupSucceeded to be True" last_succeeded
ok "the Cluster's LastBackupSucceeded is True"
recovery_window() {
  _w=$(k -n "$ns" get "$r_store/$db" -o json | jq -r --arg s "$db" '.status.serverRecoveryWindow[$s] | "\(.firstRecoverabilityPoint // "") \(.lastSuccessfulBackupTime // "")"' 2>/dev/null || true)
  printf 'serverRecoveryWindow[%s] = "%s"\n' "$db" "$_w"
  case $_w in ' ' | '' | ' '*) return 1 ;; esac
  case $_w in *' ') return 1 ;; esac
}
until_ok 90 "the ObjectStore's status.serverRecoveryWindow to name a first recoverability point and a last good backup" recovery_window
ok "the ObjectStore says what can be restored: $(k -n "$ns" get "$r_store/$db" -o json | jq -c --arg s "$db" '.status.serverRecoveryWindow[$s]')"
base=$prefix/$db/base/
in_bucket() {
  _folders=$(s3_folders "$base") || return 1
  printf 'folders under %s: %s\n' "$base" "$(printf '%s' "$_folders" | tr '\n' ' ')"
  printf '%s\n' "$_folders" | grep -qxF "$base$backup_id/"
}
until_ok 60 "the base backup $backup_id in the bucket under $base" in_bucket
s3_keys "$base$backup_id/" > "$work/base-keys"
grep -q '/backup\.info$' "$work/base-keys" || die "the base backup $backup_id has no backup.info in the bucket; its objects: $(tr '\n' ' ' < "$work/base-keys")"
ok "the bucket holds the base backup: $(wc -l < "$work/base-keys" | tr -d ' ') objects under $base$backup_id/ (backup.info among them)"
s3_keys "$prefix/$db/wals/" > "$work/wal-keys"
[ -s "$work/wal-keys" ] || die "the bucket holds no WAL under $prefix/$db/wals/"
ok "the bucket holds WAL: $(wc -l < "$work/wal-keys" | tr -d ' ') objects under $prefix/$db/wals/"

step "6. a thread after the base backup: only the WAL has it"
# The newest base backup is the one a restore starts from: stop the schedule, and wait until no backup runs or waits.
k -n "$ns" patch "$r_sched/$db" --type=merge -p '{"spec":{"suspend":true}}' >/dev/null
sleep 5
no_backup_running() {
  _active=$(k -n "$ns" get "$r_backup" -l "cnpg.io/scheduled-backup=$db" -o json | jq -r '[.items[] | select((.status.phase // "pending") | IN("pending", "started", "running", "finalizing"))] | length')
  printf 'backups running or waiting: %s\n' "$_active"
  [ "$_active" = 0 ]
}
until_ok 180 "no Backup to be running or waiting after the schedule was suspended" no_backup_running
sleep 5
until_ok 60 "no Backup to be running or waiting (second look)" no_backup_running
newest=$(k -n "$ns" get "$r_backup" -l "cnpg.io/scheduled-backup=$db" -o json | jq -r '[.items[] | select(.status.phase == "completed")] | sort_by(.status.stoppedAt) | last | .metadata.name')
ok "the schedule is suspended; the newest completed Backup, the one a restore starts from, is $newest"
insert_thread "$id_b" 000000000002h "$title_b"
wal_b=$(psql_c "$db" 'SELECT pg_walfile_name(pg_switch_wal() - 1)')
ok "a second thread written: '$title_b'; its WAL segment is $wal_b, switched"
wal_archived() {
  _ok=$(psql_c "$db" "SELECT coalesce(last_archived_wal, '') COLLATE \"C\" >= '$wal_b' FROM pg_stat_archiver")
  printf 'segment %s archived (or a later one): %s; archived_count, failed_count, last_archived_wal: %s\n' "$wal_b" "$_ok" "$(psql_c "$db" "SELECT archived_count, failed_count, coalesce(last_archived_wal, 'none') FROM pg_stat_archiver")"
  [ "$_ok" = t ]
}
until_ok "$wal_timeout" "PostgreSQL to have archived the segment $wal_b" wal_archived
in_bucket_wal() {
  s3_keys "$prefix/$db/wals/" > "$work/wal-keys" || return 1
  printf 'WAL objects: %s\n' "$(tr '\n' ' ' < "$work/wal-keys")"
  grep -qE "/$wal_b(\\.|\$)" "$work/wal-keys"
}
until_ok 60 "the segment $wal_b in the bucket" in_bucket_wal
ok "the segment $wal_b is archived: pg_stat_archiver and the bucket say it ($(grep -E "/$wal_b(\\.|\$)" "$work/wal-keys"))"

step "7. the restore: examples/restore-cluster.yaml, as written"
k -n "$ns" apply -f "$chart/examples/restore-cluster.yaml" >/dev/null
until_ok "$restore_timeout" "the restored cluster $db_restore to be Ready" cluster_ready "$db_restore"
ok "the cluster $db_restore is Ready ($(jp "$r_cluster/$db_restore" '{.status.phase}'))"
# The plugin puts its sidecar in the pod of a cluster that only RECOVERS from a store until the cluster has a primary, and CloudNativePG then rolls the pod to drop
# it (plugin v0.15.1, internal/cnpgi/operator/lifecycle.go, shouldInjectBarmanSidecar: read, not run), so the cluster may restart once after it was first Ready:
# what is asked of it below is asked until it answers.
restored_threads() {
  _got=$(psql_c "$db_restore" 'SET ROLE orchestrator; SELECT title FROM threads ORDER BY id') || return 1
  printf '%s\n' "$_got"
  [ "$_got" = "$(printf '%s\n%s' "$title_a" "$title_b")" ]
}
until_ok 180 "SELECT title FROM threads on the restored cluster to return both titles" restored_threads
ok "SELECT title FROM threads, as the owner, on the restored cluster: '$title_a' (from the base backup) and '$title_b' (from the WAL)"
restored_shape() {
  _tables=$(psql_c "$db_restore" "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'") || return 1
  _owner=$(psql_c "$db_restore" "SELECT tableowner FROM pg_tables WHERE schemaname = 'public' AND tablename = 'threads'") || return 1
  printf 'tables %s (the source has %s), owner of threads: %s\n' "$_tables" "$tables_source" "$_owner"
  [ "$_tables" = "$tables_source" ] && [ "$_owner" = orchestrator ]
}
until_ok 60 "the restored database to have the source's tables, owned by orchestrator" restored_shape
ok "the restored database has the $tables_source tables of the source, owned by orchestrator"
containers=$(pod_containers "$(jp "$r_cluster/$db_restore" '{.status.currentPrimary}')" 2>/dev/null || true)
note "the restored primary's containers now (information only): ${containers:-none read}"

step "8. the restore archives nothing"
[ -z "$(jp "$r_cluster/$db_restore" '{.spec.plugins}')" ] || die "the restored Cluster has a plugins entry"
expected_folders=$prefix/$db/
folders=$(s3_folders "$prefix/")
[ "$folders" = "$expected_folders" ] || die "the bucket has a server folder besides the source's after the restore: folders under $prefix/ are '$folders', expected only $expected_folders"
switch_restored() { psql_c "$db_restore" 'SELECT pg_walfile_name(pg_switch_wal() - 1)'; }
until_ok 120 "a WAL switch on the restored cluster" switch_restored
sleep 30
folders=$(s3_folders "$prefix/")
[ "$folders" = "$expected_folders" ] || die "the restored cluster wrote to the bucket: folders under $prefix/ are '$folders', expected only $expected_folders"
[ "$(cond "$r_cluster/$db_restore" ContinuousArchiving)" != True ] || die "the restored cluster says it archives continuously (ContinuousArchiving is True)"
ok "after a WAL switch on the restored cluster and 30 s the bucket still holds one server folder, $expected_folders, and the restored cluster does not say it archives"

step "9. the credentials are in no log and in no object"
: > "$work/everything"
for n in "$ns" "$ns_ops" "$ns_cm"; do
  for pod in $(k -n "$n" get pods -o name 2>/dev/null || true); do
    k -n "$n" logs "$pod" --all-containers --tail=-1 >> "$work/everything" 2>/dev/null || true
  done
done
k -n "$ns" get "$r_cluster,$r_store,$r_sched,$r_backup,pods,jobs,deployments,services,events" -o yaml >> "$work/everything" 2>/dev/null || true
if [ ! -s "$work/everything" ]; then die "nothing was read for the credentials check"; fi
if grep -qF -f "$work/secrets" "$work/everything"; then
  die "a credential of this run is in a log or an object of the namespaces (the line is not printed)"
fi
ok "the access key and the secret key are in no log of the three namespaces and in no object of $ns ($(wc -l < "$work/everything" | tr -d ' ') lines read)"

printf '\nBACKUP E2E OK: the database was backed up to S3 (base backup and WAL) and examples/restore-cluster.yaml brought it back, rows from the base backup and from the WAL\n'
