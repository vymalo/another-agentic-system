#!/usr/bin/env sh
# Drift check for the mocks vendored from vymalo/another-adam-rs (ADR 0014, dev/coder/UPSTREAM).
#
#   dev/coder/check-vendored.sh
#
# 1. Every file under dev/coder/wiremock and dev/coder/git-server must equal the file at the same
#    path below dev/ in the upstream repository at the commit recorded in dev/coder/UPSTREAM.
# 2. Nothing is missing: every body file a vendored WireMock mapping names (bodyFileName) is vendored
#    too, and dev/coder/git-server holds exactly the files of dev/git-server upstream at that commit.
#    The mappings themselves are a deliberate subset (the scripted coder run only); a mapping the
#    coder starts to need upstream shows up as an unmatched request in dev/coder-e2e.sh.
# 3. compose.yaml must pin the coder image to the tag sha-<first 7 characters of that commit>, with
#    a digest, so the image and the mocks are always the same upstream commit.
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

count=0
# Sorted, so the output is stable. File names here contain no spaces.
for f in $(find dev/coder/wiremock dev/coder/git-server -type f | sort); do
  upstream=dev/${f#dev/coder/}
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

# dev/coder/git-server must hold exactly the upstream files, no more and no fewer.
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
  upstream_set=$(printf '%s' "$listed" | jq -r '.tree[] | select(.type == "blob") | .path | select(startswith("dev/git-server/"))' | sort)
  local_set=$(find dev/coder/git-server -type f | sed 's|^dev/coder/|dev/|' | sort)
  if [ "$upstream_set" = "$local_set" ]; then
    echo "ok    dev/coder/git-server holds every file of dev/git-server"
  else
    echo "FAIL  dev/coder/git-server and dev/git-server at $commit hold different files:" >&2
    printf '%s\n' "$upstream_set" > "$tmp"
    printf '%s\n' "$local_set" | diff "$tmp" - | sed -n 's/^< /  missing here: /p; s/^> /  not upstream: /p' >&2
    fail=1
  fi
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
