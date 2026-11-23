#!/bin/sh
# internal-objects.sh: from a render on stdin, the two documents the run routes add (ADR 0084 decision 5): the
# orchestrator's Service (its internal port) and its NetworkPolicy, without the two labels that move with the chart's own
# version. What render-check.sh compares with tests/golden/internal-listener.yaml; see there for how a golden is made.
awk '
  function flush() {
    if (buf ~ /(^|\n)kind: (Service|NetworkPolicy)\n/ && buf ~ /app\.kubernetes\.io\/component: orchestrator\n/) printf "---\n%s", buf
    buf = ""
  }
  /^---$/ { flush(); next }
  /^    (helm\.sh\/chart|app\.kubernetes\.io\/version):/ { next }
  { buf = buf $0 "\n" }
  END { flush() }'
