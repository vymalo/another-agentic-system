#!/usr/bin/env sh
# Drift check for the mocks vendored from vymalo/another-adam-rs (ADR 0014, dev/coder/UPSTREAM).
#
#   dev/coder/check-vendored.sh
#
# 1. Every file under dev/coder/wiremock and dev/coder/git-server must equal the file at the same
#    path below dev/ in the upstream repository at the commit recorded in dev/coder/UPSTREAM, and
#    every file under dev/coder/agent (the folder the coder reads at run time, mounted at
#    /etc/adam/agent) must equal the file at the same path below bin/adam-coder/agent/ upstream.
# 2. Nothing is missing: every body file a vendored WireMock mapping names (bodyFileName) is vendored
#    too, and dev/coder/git-server and dev/coder/agent hold exactly the files of dev/git-server and
#    bin/adam-coder/agent upstream at that commit. The mappings themselves are a deliberate subset
#    (the scripted coder run only); a mapping the coder starts to need upstream shows up as an
#    unmatched request in dev/coder-e2e.sh.
# 3. compose.yaml must pin the coder image to the tag sha-<first 7 characters of that commit>, with
#    a digest, so the image, the mocks and the agent folder are always the same upstream commit.
#
# Environment: RAW_BASE overrides https://raw.githubusercontent.com and GITHUB_API overrides
# https://api.github.com (for a mirror or a test); GITHUB_TOKEN, when set, authenticates the one API
# call (the anonymous rate limit is low on shared runners).
# Needs: curl, cmp, find, sed, grep, jq and network access. Exit status 0 when nothing drifted.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

field() { sed -n "s/^$1=//p" dev/coder/UPSTREAM | head -n 1; }
repo=$(field repo)
commit=$(field commit)
base=${RAW_BASE:-https://raw.githubusercontent.com}
fail=0

case $commit in
  *[!0-9a-f]* | "") echo "FAIL  dev/coder/UPSTREAM: commit '$commit' is not a full lower-case sha" >&2; exit 1 ;;
esac
[ "${#commit}" -eq 40 ] || { echo "FAIL  dev/coder/UPSTREAM: commit '$commit' is not 40 characters" >&2; exit 1; }
[ -n "$repo" ] || { echo "FAIL  dev/coder/UPSTREAM: no repo" >&2; exit 1; }

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

# upstream_path FILE: the path upstream of a vendored file. The agent folder lives below
# bin/adam-coder/agent upstream, everything else below dev/ at the same relative path.
upstream_path() {
  case $1 in
    dev/coder/agent/*) echo "bin/adam-coder/agent/${1#dev/coder/agent/}" ;;
    *) echo "dev/${1#dev/coder/}" ;;
  esac
}

count=0
# Sorted, so the output is stable. File names here contain no spaces.
for f in $(find dev/coder/wiremock dev/coder/git-server dev/coder/agent -type f | sort); do
  upstream=$(upstream_path "$f")
  url=$base/$repo/$commit/$upstream
  count=$((count + 1))
  if ! curl -fsSL --retry 3 --retry-delay 2 -o "$tmp" "$url"; then
    echo "FAIL  $f: cannot fetch $url" >&2
    fail=1
  elif cmp -s "$f" "$tmp"; then
    echo "ok    $f"
  else
    echo "FAIL  $f differs from $upstream at $commit" >&2
    fail=1
  fi
done
[ "$count" -gt 0 ] || { echo "FAIL  no vendored files found" >&2; exit 1; }

# Every body file a vendored mapping names must be vendored next to it.
for m in dev/coder/wiremock/*/mappings/*.json; do
  files=${m%/mappings/*}/__files
  grep -o '"bodyFileName"[[:space:]]*:[[:space:]]*"[^"]*"' "$m" | sed 's/.*"\([^"]*\)"$/\1/' | sort -u > "$tmp"
  while IFS= read -r b; do
    if [ -f "$files/$b" ]; then
      echo "ok    $m: $b is vendored"
    else
      echo "FAIL  $m names $b, but $files/$b is not vendored" >&2
      fail=1
    fi
  done < "$tmp"
done

# dev/coder/git-server and dev/coder/agent must hold exactly the upstream files, no more and no fewer.
api=${GITHUB_API:-https://api.github.com}
tree_url="$api/repos/$repo/git/trees/$commit?recursive=1"
if [ -n "${GITHUB_TOKEN:-}" ]; then
  listed=$(curl -fsSL --retry 3 --retry-delay 2 -H "Authorization: Bearer $GITHUB_TOKEN" "$tree_url") || listed=
else
  listed=$(curl -fsSL --retry 3 --retry-delay 2 "$tree_url") || listed=
fi
if [ -z "$listed" ]; then
  echo "FAIL  cannot list $repo at $commit ($tree_url)" >&2
  fail=1
elif [ "$(printf '%s' "$listed" | jq -r '.truncated')" != "false" ]; then
  echo "FAIL  the tree of $repo at $commit is truncated; cannot check completeness" >&2
  fail=1
else
  # same_files LOCAL_DIR UPSTREAM_DIR: the files below LOCAL_DIR are exactly those below UPSTREAM_DIR upstream.
  same_files() {
    upstream_set=$(printf '%s' "$listed" | jq -r --arg p "$2/" '.tree[] | select(.type == "blob") | .path | select(startswith($p))' | sort)
    local_set=$(find "$1" -type f | while IFS= read -r lf; do upstream_path "$lf"; done | sort)
    if [ "$upstream_set" = "$local_set" ]; then
      echo "ok    $1 holds every file of $2"
    else
      echo "FAIL  $1 and $2 at $commit hold different files:" >&2
      printf '%s\n' "$upstream_set" > "$tmp"
      printf '%s\n' "$local_set" | diff "$tmp" - | sed -n 's/^< /  missing here: /p; s/^> /  not upstream: /p' >&2
      fail=1
    fi
  }
  same_files dev/coder/git-server dev/git-server
  same_files dev/coder/agent bin/adam-coder/agent
fi

short=$(printf '%s' "$commit" | cut -c1-7)
if grep -Eq "^[[:space:]]+image: ghcr\.io/vymalo/another-adam-rs/coder:sha-$short@sha256:[0-9a-f]{64}([[:space:]]|\$)" compose.yaml; then
  echo "ok    compose.yaml pins coder:sha-$short@sha256:..."
else
  echo "FAIL  compose.yaml does not pin ghcr.io/vymalo/another-adam-rs/coder:sha-$short@sha256:<digest>" >&2
  fail=1
fi

[ "$fail" -eq 0 ] && echo "vendored files match $repo@$short"
exit "$fail"
