#!/bin/sh
# Runs `orchestrator --print-config` on the configuration the chart renders: the file and the agents file of the
# ConfigMap, with dummy secret files where the configuration says `{ file }` and dummy variables where it says `{ env }` (the
# agents' bearers). It opens no connection, and reads no real secret. A configuration the orchestrator refuses (exit 78) fails
# CI here and not at the first sync.
#
#   deploy/chart/tests/print-config.sh <values-file> docker <image>      the image the chart deploys (CI)
#   deploy/chart/tests/print-config.sh <values-file> bin <orchestrator>  a local binary
#
# <values-file> may be several, separated by commas (helm's own layering: later ones win).
#
# `docker` mounts the files where the pod does. `bin` rewrites /run/secrets/ into a temporary directory first, because a
# local run cannot write there. Needs helm, awk and sed.
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: $0 <values-file> docker <image> | $0 <values-file> bin <orchestrator>" >&2
  exit 2
fi
values=$1 mode=$2 target=$3
chart="$(dirname "$0")/.."
work=$(mktemp -d)
trap 'chmod -R u+rwX "$work" 2>/dev/null || true; rm -rf "$work"' EXIT

# Each comma-separated path is one `-f` (no path of ours has a comma or a space).
set --
for v in $(printf '%s' "$values" | tr ',' ' '); do set -- "$@" -f "$v"; done
helm template another-agentic-system "$chart" --namespace another-agentic-system "$@" \
  --show-only templates/orchestrator-configmap.yaml > "$work/cm.yaml"

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
extract agents.yaml "$work/etc/agents.yaml"

# One dummy file per `{ file: /run/secrets/... }` of the configuration (at least 32 bytes: the thread tools' key needs it).
dummy='dummy-secret-for-print-config-only-0123456789abcdef'
grep -o 'file: /run/secrets/[^ }]*' "$work/etc/config.yaml" | sed 's/^file: //' | while read -r f; do
  rel=${f#/run/secrets/}
  mkdir -p "$work/secrets/$(dirname "$rel")"
  printf '%s\n' "$dummy" > "$work/secrets/$rel"
done

# One dummy variable per `tokenEnv` of the agents file.
envs=$(sed -n 's/^ *tokenEnv: //p' "$work/etc/agents.yaml")

case "$mode" in
  docker)
    set -- docker run --rm --user 65532:65532 \
      -v "$work/etc:/etc/orchestrator:ro" -v "$work/secrets/orchestrator:/run/secrets/orchestrator:ro" -v "$work/secrets/db:/run/secrets/db:ro" \
      -e ORCH_CONFIG_FILE=/etc/orchestrator/config.yaml
    for e in $envs; do set -- "$@" -e "$e=$dummy"; done
    chmod -R a+rX "$work"
    "$@" "$target" --print-config
    ;;
  bin)
    sed -i "s#/run/secrets/#$work/secrets/#g" "$work/etc/config.yaml"
    for e in $envs; do export "$e=$dummy"; done
    ORCH_CONFIG_FILE="$work/etc/config.yaml" "$target" --print-config
    ;;
  *) echo "mode must be docker or bin" >&2; exit 2 ;;
esac
