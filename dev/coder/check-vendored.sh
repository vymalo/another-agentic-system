#!/usr/bin/env sh
# Drift check for the mocks vendored from vymalo/another-adam-rs (ADR 0014, dev/coder/UPSTREAM).
#
#   dev/coder/check-vendored.sh
#
# 1. Every file under dev/coder/wiremock and dev/coder/git-server must equal the file at the same
#    path below dev/ in the upstream repository at the commit recorded in dev/coder/UPSTREAM.
# 2. compose.yaml must pin the coder image to the tag sha-<first 7 characters of that commit>, with
#    a digest, so the image and the mocks are always the same upstream commit.
#
# Environment: RAW_BASE overrides https://raw.githubusercontent.com (for a mirror or a test).
# Needs: curl, cmp, find, sed, grep and network access. Exit status 0 when nothing drifted.
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

short=$(printf '%s' "$commit" | cut -c1-7)
if grep -Eq "^[[:space:]]+image: ghcr\.io/vymalo/another-adam-rs/coder:sha-$short@sha256:[0-9a-f]{64}([[:space:]]|\$)" compose.yaml; then
  echo "ok    compose.yaml pins coder:sha-$short@sha256:..."
else
  echo "FAIL  compose.yaml does not pin ghcr.io/vymalo/another-adam-rs/coder:sha-$short@sha256:<digest>" >&2
  fail=1
fi

[ "$fail" -eq 0 ] && echo "vendored files match $repo@$short"
exit "$fail"
