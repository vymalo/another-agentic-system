#!/bin/sh
# Set `<component>.image.tag` in a values file to a new tag and show the change.
#
#   bump-tag.sh <values.yaml> <component> <tag>      component: orchestrator | web
#
# Prints `unchanged` (and leaves the file alone) when the tag is already current, so a re-run is a no-op and the workflow
# never commits twice. Only the `tag:` key inside `<component>:` / `image:` is touched; every other line is preserved
# byte for byte. Used by the real bump jobs (orchestrator.yml, web.yml) and by their dry runs on pull requests, so the dry
# run proves the logic that will run on main. The pattern of vymalo/another-adam-rs deploy/coder/bump-tag.sh, one level deeper.
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: $0 <values.yaml> <orchestrator|web> <tag>" >&2
  exit 2
fi
file=$1
component=$2
tag=$3

case "$component" in
  orchestrator|web) ;;
  *) echo "refusing component '$component': expected orchestrator or web" >&2; exit 2 ;;
esac
case "$tag" in
  sha-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
  *) echo "refusing tag '$tag': expected sha-<7 hex digits>" >&2; exit 2 ;;
esac

# in_c: inside the top-level `<component>:` block; in_i: inside its `  image:` block.
current=$(awk -v c="$component" '
  $0 == c ":" { in_c = 1; in_i = 0; next }
  in_c && /^[^ #]/ { in_c = 0; in_i = 0 }
  in_c && /^  [^ #]/ { in_i = ($0 ~ /^  image:/) }
  in_c && in_i && /^    tag:/ { sub(/^    tag:[ ]*/, ""); sub(/[ ]*(#.*)?$/, ""); gsub(/"/, ""); print; exit }
' "$file")
if [ -z "$current" ]; then
  echo "no $component.image.tag found in $file" >&2
  exit 1
fi
if [ "$current" = "$tag" ]; then
  echo "unchanged: $component.image.tag is already $tag"
  exit 0
fi

tmp=$(mktemp)
awk -v c="$component" -v tag="$tag" '
  $0 == c ":" { in_c = 1; in_i = 0; print; next }
  in_c && /^[^ #]/ { in_c = 0; in_i = 0 }
  in_c && /^  [^ #]/ { in_i = ($0 ~ /^  image:/) }
  in_c && in_i && !done && /^    tag:/ { print "    tag: " tag; done = 1; next }
  { print }
' "$file" > "$tmp"
cat "$tmp" > "$file"
rm -f "$tmp"
echo "changed: $component.image.tag $current -> $tag"
