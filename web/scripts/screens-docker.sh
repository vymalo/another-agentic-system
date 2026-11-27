#!/bin/sh
# Generates the screenshots of e2e/__screens__/ in web/Dockerfile's `screens` target and compares them with the committed set.
#   sh web/scripts/screens-docker.sh check    # exit 1 when a still is missing or new, has other dimensions, or has more than 0.05 % of its pixels beyond 8/255
#   sh web/scripts/screens-docker.sh write    # rewrites the committed set from the image
set -eu
mode="${1:-check}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
docker build -f "$root/web/Dockerfile" --target screens -t web-screens "$root"
chmod 777 "$out"
docker run --rm -v "$out:/app/web/e2e/__screens__" web-screens
dir="$root/web/e2e/__screens__"
# SCREENS_KEEP=<dir>: a copy of what the image drew, for CI to upload when the comparison fails (the set is still only ever written by `write`)
if [ -n "${SCREENS_KEEP:-}" ]; then mkdir -p "$SCREENS_KEEP" && cp "$out"/*.png "$SCREENS_KEEP"/; fi
if [ "$mode" = write ]; then
  cp "$out"/*.png "$dir"/
  echo "wrote $(ls "$out" | wc -l) images"
  exit 0
fi
# The comparison runs in the same image (ImageMagick is there, never rely on the host's): a still passes when at most 0.05 % of its
# pixels differ by more than 8/255; a missing still or changed dimensions fail (scripts/screens-compare.sh, web/README.md).
docker run --rm -v "$out:/generated:ro" -v "$dir:/committed:ro" --entrypoint sh web-screens scripts/screens-compare.sh /generated /committed
