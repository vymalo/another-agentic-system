#!/bin/sh
# Pin the orchestrator's and the web's images, by version AND digest, in the values file of a COPY of this chart that a release
# packages (ADR 0106, .github/workflows/release.yml), and show the change.
#
#   release-pin.sh <values.yaml> <X.Y.Z> <orchestrator sha256:...> <web sha256:...>
#
# It sets `orchestrator.image.tag` and `web.image.tag` to X.Y.Z and writes `orchestrator.image.digest` and `web.image.digest` on the
# line after each (replacing a digest that is there); every other line is kept byte for byte. It REFUSES a file that git tracks: in
# git those tags are `sha-<7>` and are written by bump-tag.sh only (on main, after every green image build). A release version lives
# only in the packaged chart, so no commit on main ever names one. The awk program is bump-tag.sh's shape (`<component>:` at the left
# margin, then `  image:`, then `    tag:`) and, like it, touches nothing in a comment and no `tag:` of another block.
# Exit 2 for input it refuses, 1 when a tag is not found exactly once.
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: $0 <values.yaml> <X.Y.Z> <orchestrator sha256:...> <web sha256:...>" >&2
  exit 2
fi
file=$1
version=$2
orch_digest=$3
web_digest=$4

if ! printf '%s\n' "$version" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+'; then
  echo "refusing version '$version': expected X.Y.Z (the release tag without its v)" >&2
  exit 2
fi
for d in "$orch_digest" "$web_digest"; do
  if ! printf '%s\n' "$d" | grep -Eqx 'sha256:[0-9a-f]{64}'; then
    echo "refusing digest '$d': expected sha256: and 64 hex digits" >&2
    exit 2
  fi
done
if [ ! -f "$file" ]; then
  echo "no such file: $file" >&2
  exit 2
fi
# In git the tags are bump-tag.sh's alone (`git ls-files --error-unmatch` succeeds only for a tracked file).
dir=$(cd "$(dirname "$file")" && pwd)
if git -C "$dir" ls-files --error-unmatch -- "$(basename "$file")" > /dev/null 2>&1; then
  echo "refusing $file: git tracks it, and in git the image tags are sha-<7>, written by bump-tag.sh only; pin a copy (ADR 0106)" >&2
  exit 2
fi

# top: the top-level key of the line's block; in_i: inside its `  image:` block.
# shellcheck disable=SC2016
prog='
  /^[^ #]/ { top = $0; sub(/:.*/, "", top); in_i = 0; print; next }
  (top == "orchestrator" || top == "web") && /^  [^ #]/ { in_i = ($0 ~ /^  image:/) }
  (top == "orchestrator" || top == "web") && in_i && /^    digest:/ { next }
  (top == "orchestrator" || top == "web") && in_i && /^    tag:/ {
    n[top]++
    print "    tag: \"" version "\""
    print "    digest: \"" (top == "orchestrator" ? od : wd) "\""
    next
  }
  { print }
  END {
    if (n["orchestrator"] != 1 || n["web"] != 1) {
      printf "found orchestrator.image.tag %d and web.image.tag %d times; want each once\n", n["orchestrator"], n["web"] > "/dev/stderr"
      exit 1
    }
  }
'
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
if ! awk -v version="$version" -v od="$orch_digest" -v wd="$web_digest" "$prog" "$file" > "$tmp"; then
  echo "not pinned: $file is left as it was" >&2
  exit 1
fi
diff "$file" "$tmp" || true
cat "$tmp" > "$file"
echo "pinned: orchestrator.image and web.image to $version, by digest"
