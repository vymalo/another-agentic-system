#!/bin/sh
# Generates the screenshots of e2e/__screens__/ in web/Dockerfile's `screens` target and compares them with the committed set.
#   sh web/scripts/screens-docker.sh check    # exit 1 when any image differs, is missing or is new; prints the counts
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
if [ "$mode" = write ]; then
  cp "$out"/*.png "$dir"/
  echo "wrote $(ls "$out" | wc -l) images"
  exit 0
fi
total=0; same_dims=0; changed_dims=0; missing=0
for f in "$out"/*.png; do
  n="$(basename "$f")"; total=$((total + 1))
  if [ ! -f "$dir/$n" ]; then echo "new: $n"; missing=$((missing + 1)); continue; fi
  cmp -s "$f" "$dir/$n" && continue
  # PNG width and height: bytes 16 to 23 of the file
  a="$(od -An -tx1 -j16 -N8 "$f" | tr -d ' \n')"; b="$(od -An -tx1 -j16 -N8 "$dir/$n" | tr -d ' \n')"
  if [ "$a" = "$b" ]; then echo "differs, same dimensions: $n"; same_dims=$((same_dims + 1))
  else echo "differs, changed dimensions: $n"; changed_dims=$((changed_dims + 1)); fi
done
for f in "$dir"/*.png; do [ -f "$out/$(basename "$f")" ] || { echo "not generated: $(basename "$f")"; missing=$((missing + 1)); }; done
echo "$total generated: $same_dims differ with the same dimensions, $changed_dims with changed dimensions, $missing new or not generated"
[ $((same_dims + changed_dims + missing)) -eq 0 ]
