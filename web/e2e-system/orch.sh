#!/bin/sh
# Starts the orchestrator under test (`$ORCH_BIN`) and records its pid and log under `.run/`,
# so a spec can SIGKILL it (restart.spec.ts) and CI can upload the log. `exec` keeps the pid.
# `ORCH_LOG_APPEND=1` (a restart) appends to the log instead of starting a new one.
set -eu
dir=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$dir/.run"
echo $$ > "$dir/.run/orchestrator.pid"
if [ "${ORCH_LOG_APPEND:-}" = "1" ]; then
  exec "${ORCH_BIN:?ORCH_BIN must point at the orchestrator binary}" >> "$dir/.run/orchestrator.log" 2>&1
fi
exec "${ORCH_BIN:?ORCH_BIN must point at the orchestrator binary}" > "$dir/.run/orchestrator.log" 2>&1
