#!/bin/sh
# db-objects.sh: from a render on stdin, the documents that make the databases (the CNPG Cluster, every Database and every
# ExternalSecret that builds a basic-auth database Secret), without the two labels that move with the chart's own version.
# What render-check.sh compares with tests/golden/*.yaml; see there for how a golden is made.
awk '
  function flush() {
    if (buf ~ /(^|\n)kind: (Cluster|Database)\n/ || (buf ~ /(^|\n)kind: ExternalSecret\n/ && buf ~ /kubernetes\.io\/basic-auth/)) printf "---\n%s", buf
    buf = ""
  }
  /^---$/ { flush(); next }
  /^    (helm\.sh\/chart|app\.kubernetes\.io\/version):/ { next }
  { buf = buf $0 "\n" }
  END { flush() }'
