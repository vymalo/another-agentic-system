#!/usr/bin/env sh
# Asserts that dev/orchestrator.browser-auth.yaml is dev/orchestrator.yaml and nothing else but the blocks between its
# `browser-auth:begin` and `browser-auth:end` markers (ADR 0054; dev/compose.browser-auth.yaml mounts it in place of the default one while the
# orchestrator image of the default stack may not know `auth.dpop` and `auth.browser`), that those blocks are the two the orchestrator reads,
# and that the default file has neither. No docker needed.
#
#   dev/check-browser-auth-config.sh
#
# Exit status 0 when the two files agree, 1 otherwise.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
default=$here/orchestrator.yaml
browser=$here/orchestrator.browser-auth.yaml
fail=0
ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1" >&2; fail=1; }

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
awk '/# browser-auth:begin/ { skip = 1; next } /# browser-auth:end/ { skip = 0; next } !skip' "$browser" > "$tmp"
if diff -u "$default" "$tmp" >&2; then
  ok "orchestrator.browser-auth.yaml is orchestrator.yaml plus its marked blocks"
else
  bad "orchestrator.browser-auth.yaml differs from orchestrator.yaml outside its markers (above): change both, or only the marked blocks"
fi

for key in '  dpop:' '    publicOrigins:' '  browser:' '    clientId: dev-web'; do
  if grep -Fxq "$key" "$browser"; then ok "the browser file has '$key'"; else bad "the browser file lacks '$key'"; fi
done
if grep -Eq '^  (dpop|browser):' "$default"; then
  bad "orchestrator.yaml has auth.dpop or auth.browser: the default stack would need the orchestrator of ADR 0054 (then delete the browser file, this script and the override's orchestrator entry)"
else
  ok "orchestrator.yaml has neither auth.dpop nor auth.browser"
fi
if [ "$(grep -c '# browser-auth:begin' "$browser")" -eq "$(grep -c '# browser-auth:end' "$browser")" ]; then
  ok "every begin marker has an end marker"
else
  bad "the begin and end markers do not match"
fi
[ "$fail" -eq 0 ] || exit 1
