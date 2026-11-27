#!/usr/bin/env sh
# Asserts that dev/orchestrator.provisioner.yaml is dev/orchestrator.yaml and nothing else but the blocks between its
# `provisioner:begin` and `provisioner:end` markers (ANR-806; dev/compose.provisioner.yaml mounts it in place of the default one, because the
# default stack has no platform to ask), that the marked blocks hold the `provisioner` section the scenario relies on, and that the default file
# has none. No docker needed.
#
#   dev/check-provisioner-config.sh
#
# Exit status 0 when the two files agree, 1 otherwise.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
default=$here/orchestrator.yaml
derived=$here/orchestrator.provisioner.yaml
fail=0
ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1" >&2; fail=1; }

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
awk '/# provisioner:begin/ { skip = 1; next } /# provisioner:end/ { skip = 0; next } !skip' "$derived" > "$tmp"
if diff -u "$default" "$tmp" >&2; then
  ok "orchestrator.provisioner.yaml is orchestrator.yaml plus its marked blocks"
else
  bad "orchestrator.provisioner.yaml differs from orchestrator.yaml outside its markers (above): change both, or only the marked blocks"
fi

for key in '    userClaim: sub' 'provisioner:' '  url: http://mock-platform:8080' '  kinds: [coder]' '  readyTimeoutSecs: 6'; do
  if grep -Fxq "$key" "$derived"; then ok "the derived file has '$key'"; else bad "the derived file lacks '$key'"; fi
done
if grep -Eq '^provisioner:' "$default"; then
  bad "orchestrator.yaml has a provisioner section: the default stack has no platform to ask (then delete the derived file, this script and the override's orchestrator entry)"
else
  ok "orchestrator.yaml has no provisioner section"
fi
if [ "$(grep -c '# provisioner:begin' "$derived")" -eq "$(grep -c '# provisioner:end' "$derived")" ]; then
  ok "every begin marker has an end marker"
else
  bad "the begin and end markers do not match"
fi
[ "$fail" -eq 0 ] || exit 1
