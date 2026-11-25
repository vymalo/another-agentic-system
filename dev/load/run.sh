#!/usr/bin/env sh
# The load test of the dev stack, one command (mocks only, no secret): dev/load/run.sh
# Ramp: STEPS=10,50,100,200 concurrent runs (the first CI-sized run: STEPS=10 HOLD_SECS=10). See dev/README.md "Load test".
set -eu
command -v node >/dev/null 2>&1 || { echo "FAIL node (22 or later) is needed" >&2; exit 2; }
exec node "$(dirname "$0")/load.mjs"
