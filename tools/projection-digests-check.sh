#!/usr/bin/env sh
# The projection's digest table may change a line it already had only when its version rises
# (docs/api/history.md rule 7, orchestrator/crates/agui-projection/tests/projection-digests.txt).
#
# The first line is `projection <n>`; every other line is `<golden> <sha256 of its frames>`. Against
# the base ref: a line that was there and is gone or different needs a higher <n>; a line that is
# new, or a table that did not exist, passes. A golden that is deleted is a change of the table too.
#
# Usage: tools/projection-digests-check.sh <base-ref>       (CI: origin/<base branch>)
# Exit 0 = fine; exit 1 = a pinned line changed and the version did not rise.

set -eu

TABLE=orchestrator/crates/agui-projection/tests/projection-digests.txt
base=${1:?usage: $0 <base-ref>}

if ! git cat-file -e "$base:$TABLE" 2>/dev/null; then
  echo "projection digests: no table at $base, nothing to compare."
  exit 0
fi

old=$(mktemp)
trap 'rm -f "$old"' EXIT
git show "$base:$TABLE" >"$old"

old_version=$(sed -n '1s/^projection //p' "$old")
new_version=$(sed -n '1s/^projection //p' "$TABLE")
if [ -z "$old_version" ] || [ -z "$new_version" ]; then
  echo "::error::the first line of $TABLE must be 'projection <n>'." >&2
  exit 1
fi

# lines that were there (not the version line) and are not there now, unchanged
changed=$(tail -n +2 "$old" | while IFS= read -r line; do
  grep -qxF -- "$line" "$TABLE" || echo "$line"
done)

if [ -n "$changed" ] && [ "$new_version" -le "$old_version" ]; then
  echo "::error::a pinned line of $TABLE changed or went, and PROJECTION_VERSION stayed at $old_version. The frames written for an event already in a log changed: raise the version (crates/agui-projection/src/history.rs) and the first line of the table, or undo the change." >&2
  echo "$changed" >&2
  exit 1
fi
echo "projection digests: ok (version $old_version -> $new_version)."
