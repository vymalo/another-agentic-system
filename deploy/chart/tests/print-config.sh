#!/bin/sh
# Runs `orchestrator --print-config` on the configuration the chart renders: the file and the agents file of the
# ConfigMap, with dummy secret files where the configuration says `{ file }` and dummy variables where it says `{ env }` (the
# agents' bearers). It opens no connection, and reads no real secret. A configuration the orchestrator refuses (exit 78) fails
# CI here and not at the first sync.
#
#   deploy/chart/tests/print-config.sh <values-file> docker <image>      the image the chart deploys (CI)
#   deploy/chart/tests/print-config.sh <values-file> bin <orchestrator>  a local binary
#   deploy/chart/tests/print-config.sh <values-file> docker <image> job  the same, but run the command of the migration Job instead
#
# `job` (ADR 0058, A5.4) runs the Job's own arguments (`artifacts migrate --from fs --to s3 --fs-root ...`) as the rendered Job has them, on the
# Job's OWN rendered configuration (the ConfigMap of `templates/artifacts-migrate.yaml`, which says `s3` even while the orchestrator's says `fs`),
# with dummy files for the Job's two S3 credentials and for nothing else (the pod has no other secret, and the command must not read one), and an EMPTY
# directory where the Job mounts the claim: so it loads the store alone, builds it, reads the empty directory and prints a report of no files,
# without a connection to any bucket. An empty directory is exit 66 and `"complete":false` unless the render has `migrate.allowEmpty`
# (tests/migrate-allow-empty.values.yaml), which is what a caller that wants exit 0 layers on. An image that does not have the command, or
# `--allow-empty`, exits 2, which is why CI runs this only for a tag at or after the commit that built them. The values must turn `migrate.enabled` on.
#
# <values-file> may be several, separated by commas (helm's own layering: later ones win).
#
# `docker` mounts the files where the pod does. `bin` rewrites /run/secrets/ into a temporary directory first, because a
# local run cannot write there. Needs helm, awk and sed.
set -eu

if [ "$#" -ne 3 ] && ! { [ "$#" -eq 4 ] && [ "$4" = job ]; }; then
  echo "usage: $0 <values-file> docker <image> [job] | $0 <values-file> bin <orchestrator> [job]" >&2
  exit 2
fi
values=$1 mode=$2 target=$3 job=${4:-}
chart="$(dirname "$0")/.."
work=$(mktemp -d)
trap 'chmod -R u+rwX "$work" 2>/dev/null || true; rm -rf "$work"' EXIT

# Each comma-separated path is one `-f` (no path of ours has a comma or a space).
set --
for v in $(printf '%s' "$values" | tr ',' ' '); do set -- "$@" -f "$v"; done
# The file the configuration comes from: the orchestrator's ConfigMap, or for `job` the Job's own (the same `config.yaml` key).
if [ "$job" = job ]; then source=templates/artifacts-migrate.yaml; else source=templates/orchestrator-configmap.yaml; fi
helm template another-agentic-system "$chart" --namespace another-agentic-system "$@" \
  --show-only "$source" > "$work/cm.yaml"

# The data keys are literal blocks indented by four spaces under a key at two.
extract() { # extract <key> <out>
  awk -v k="  $1: |" '
    $0 == k { on = 1; next }
    on && /^  [^ ]/ { on = 0 }
    on && /^---/ { on = 0 }
    on { sub(/^    /, ""); print }' "$work/cm.yaml" > "$2"
  [ -s "$2" ] || { echo "no $1 in the rendered ConfigMap" >&2; exit 1; }
}
mkdir -p "$work/etc" "$work/secrets/orchestrator" "$work/secrets/db"
extract config.yaml "$work/etc/config.yaml"
if [ "$job" = job ]; then : > "$work/etc/agents.yaml"; else extract agents.yaml "$work/etc/agents.yaml"; fi

# One dummy file per `{ file: /run/secrets/... }` of the configuration (at least 32 bytes: the thread tools' key needs it; a URL
# for the model's address).
dummy='dummy-secret-for-print-config-only-0123456789abcdef'
# The Job's configuration names every file of the orchestrator's (it is that configuration with `artifacts` replaced), but its pod has the two S3
# credentials only: the others are not made, so a command that read one would fail here as it would in the cluster.
grep -o 'file: /run/secrets/[^ }]*' "$work/etc/config.yaml" | sed 's/^file: //' | while read -r f; do
  rel=${f#/run/secrets/}
  if [ "$job" = job ]; then case "$rel" in orchestrator/artifacts-s3-*) ;; *) continue ;; esac; fi
  mkdir -p "$work/secrets/$(dirname "$rel")"
  case "$rel" in
    # The model's address (`model.baseUrlFromSecret`) is checked as a URL once read: a dummy that is one.
    */model-base-url) printf '%s\n' 'https://model.invalid/v1' > "$work/secrets/$rel" ;;
    # The keys of the stored secrets (ADR 0046, slice 3) must read as 64 hexadecimal characters (`openssl rand -hex 32`): a dummy that does.
    */user-secrets-key-*) printf '%s\n' '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' > "$work/secrets/$rel" ;;
    # The share links' key is refused when it equals the thread tools' key (ADR 0040): another dummy.
    */sharing-secret) printf '%s\n' 'another-dummy-for-print-config-only-fedcba9876543210' > "$work/secrets/$rel" ;;
    *) printf '%s\n' "$dummy" > "$work/secrets/$rel" ;;
  esac
done

# The Job's arguments, as rendered, and an empty directory for its claim.
jobargs=
if [ "$job" = job ]; then
  helm template another-agentic-system "$chart" --namespace another-agentic-system "$@" \
    --show-only templates/artifacts-migrate.yaml > "$work/job.yaml"
  jobargs=$(sed -n '/^          args:$/,/^          env:$/p' "$work/job.yaml" | sed -n 's/^ *- //p' | tr '\n' ' ')
  case "$jobargs" in "artifacts migrate "*) ;; *) echo "no migration Job in the render (is orchestrator.artifacts.migrate.enabled on?)" >&2; exit 1 ;; esac
  mkdir -p "$work/empty"
fi

# One dummy variable per `tokenEnv` of the agents file.
envs=$(sed -n 's/^ *tokenEnv: //p' "$work/etc/agents.yaml")
# And one per `{ env: NAME }` of the configuration itself: with the database behind a pooler (`database.pooler.enabled`) the two URLs are variables the
# Deployment composes. The orchestrator refuses `directUrl` equal to `url`, so the two dummies are different URLs, as the real ones are.
cfgenvs=$(sed -n 's/^.*{ env: \([A-Za-z0-9_]*\) }.*$/\1/p' "$work/etc/config.yaml")
dummy_env() { # dummy_env NAME: the value of NAME's dummy
  case $1 in
    ORCH_DB_POOLED_URL) printf '%s' 'postgresql://dummy:dummy@pooler.invalid:5432/orchestrator' ;;
    ORCH_DB_DIRECT_URL) printf '%s' 'postgresql://dummy:dummy@db-rw.invalid:5432/orchestrator' ;;
    *) printf '%s' "$dummy" ;;
  esac
}

case "$mode" in
  docker)
    set -- docker run --rm --user 65532:65532 \
      -v "$work/etc:/etc/orchestrator:ro" -v "$work/secrets/orchestrator:/run/secrets/orchestrator:ro" -v "$work/secrets/db:/run/secrets/db:ro" \
      -e ORCH_CONFIG_FILE=/etc/orchestrator/config.yaml
    for e in $envs; do set -- "$@" -e "$e=$dummy"; done
    for e in $cfgenvs; do set -- "$@" -e "$e=$(dummy_env "$e")"; done
    chmod -R a+rX "$work"
    if [ "$job" = job ]; then
      # shellcheck disable=SC2086 # the arguments are words of the rendered Job, split on purpose
      "$@" -v "$work/empty:/var/lib/orchestrator/artifacts:ro" "$target" $jobargs
    else
      "$@" "$target" --print-config
    fi
    ;;
  bin)
    sed -i "s#/run/secrets/#$work/secrets/#g" "$work/etc/config.yaml"
    for e in $envs; do export "$e=$dummy"; done
    for e in $cfgenvs; do export "$e=$(dummy_env "$e")"; done
    if [ "$job" = job ]; then
      jobargs=$(printf '%s' "$jobargs" | sed "s#/var/lib/orchestrator/artifacts#$work/empty#")
      # shellcheck disable=SC2086 # the arguments are words of the rendered Job, split on purpose
      ORCH_CONFIG_FILE="$work/etc/config.yaml" "$target" $jobargs
    else
      ORCH_CONFIG_FILE="$work/etc/config.yaml" "$target" --print-config
    fi
    ;;
  *) echo "mode must be docker or bin" >&2; exit 2 ;;
esac
