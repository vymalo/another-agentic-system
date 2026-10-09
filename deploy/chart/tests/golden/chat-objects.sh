#!/bin/sh
# chat-objects.sh: from a render on stdin, the chat agent's folder (its ConfigMap) and the checksum of it that its pods carry
# (`checksum/agent`), without the two labels that move with the chart's own version. A change to either restarts the chat on the
# next sync. What render-check.sh compares with tests/golden/chat-default.yaml: with every option of the chart off, the chat must be
# what it was (made from the chart at origin/main 4dd7c27); a change to the chat's folder regenerates it, on purpose.
awk '
  function flush() {
    if (buf ~ /(^|\n)kind: ConfigMap\n/ && buf ~ /(^|\n)  name: [a-z0-9-]*-chat-agent\n/) printf "---\n%s", buf
    buf = ""
  }
  /^---$/ { flush(); next }
  /^ +checksum\/agent: / { print "checksum/agent:" $2; next }
  /^    (helm\.sh\/chart|app\.kubernetes\.io\/version):/ { next }
  { buf = buf $0 "\n" }
  END { flush() }'
