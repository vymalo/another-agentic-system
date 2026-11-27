#!/bin/sh
# Brings a committed set of stills up to a drawn one by copying only what the bounded comparison flags (ANR-1128).
#   sh web/scripts/screens-select.sh <generated-dir> <committed-dir> [<report-file>]
# The comparison is scripts/screens-compare.sh --list-failing, run in the `screens` image (SCREENS_IMAGE, default `web-screens`;
# ImageMagick is there, never on a host). The committed directory is changed by exactly:
#   (a) a still that fails the bound (more than 0.05 % of its pixels beyond 8/255, or other dimensions) is replaced by the drawn one,
#   (b) a still that is new (drawn, not committed) is added,
#   (c) a still that is committed and no longer drawn is removed.
# A still that differs within the bound stays exactly as committed: the noise of Chromium's sub-pixel rasterisation between two
# draws is not a change (web/README.md, "screens-docker.sh"). <report-file> gets one line per still touched, with its pixel
# count and percentage as the comparison printed them (the body of the pull request); it is empty when nothing is touched.
set -eu
gen="${1:?generated dir}"; com="${2:?committed dir}"; report="${3:-/dev/null}"
image="${SCREENS_IMAGE:-web-screens}"
gen="$(cd "$gen" && pwd)"; com="$(cd "$com" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
docker run --rm -v "$gen:/generated:ro" -v "$com:/committed:ro" --entrypoint sh "$image" \
  scripts/screens-compare.sh --list-failing /generated /committed >"$work/failing" 2>"$work/detail"
cat "$work/detail" >&2
: >"$work/report"
copied=0; removed=0
while IFS= read -r n; do
  [ -n "$n" ] || continue
  if [ -f "$gen/$n" ]; then cp "$gen/$n" "$com/$n"; copied=$((copied + 1)); else rm -f "$com/$n"; removed=$((removed + 1)); fi
done <"$work/failing"
# every FAIL line of the comparison names one still that was touched above; one line each, "<name>: <what>"
sed -n -e 's/^FAIL not generated: \(.*\)$/\1: removed, no longer drawn/;t out' \
  -e 's/^FAIL new, not committed: \(.*\)$/\1: new/;t out' \
  -e 's/^FAIL changed dimensions: \([^ ]*\) (\(.*\))$/\1: changed dimensions (\2)/;t out' \
  -e 's/^FAIL //;t out' -e 'd' -e ':out' -e 'p' "$work/detail" >"$work/report"
cp "$work/report" "$report"
echo "screens-select: $copied copied (beyond the bound, new), $removed removed (no longer drawn); the rest stays as committed"
