#!/bin/sh
# Tests of deploy/chart/bump-tag.sh: it edits only `<component>.image.tag`, is idempotent, and refuses input it does not
# understand. Needs only sh, awk, cmp and diff.
#
#   sh deploy/chart/tests/bump-tag-test.sh        (from the repository root)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bump="$here/../bump-tag.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0

ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

# run <args...>: sets $out and $rc without aborting the script.
run() {
  rc=0
  out=$(sh "$bump" "$@" 2>&1) || rc=$?
}

cat > "$work/values.yaml" <<'YAML'
# A comment mentioning orchestrator: and tag: must not confuse the edit.
host: ""
orchestrator:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/orchestrator
    tag: sha-1111111 # bumped by CI
    pullPolicy: IfNotPresent
  surfaces: [agui]
web:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/web
    tag: sha-2222222
  replicas: 1
chat:
  image:
    tag: keep-me
    digest: sha256:0
  tag: also-keep-me
tag: top-level-keep-me
YAML

# 1. A new tag replaces that component's tag and nothing else.
cp "$work/values.yaml" "$work/before.yaml"
run "$work/values.yaml" orchestrator sha-abc1234
if [ "$rc" -eq 0 ] && [ "$out" = "changed: orchestrator.image.tag sha-1111111 -> sha-abc1234" ]; then
  ok "a new tag is applied and reported"
else
  bad "a new tag: rc=$rc out='$out'"
fi
if grep -q '^    tag: sha-abc1234$' "$work/values.yaml"; then ok "orchestrator.image.tag holds the new tag"; else bad "orchestrator.image.tag was not set"; fi
changed=$(diff "$work/before.yaml" "$work/values.yaml" | grep -c '^[<>]' || true)
if [ "$changed" -eq 2 ]; then ok "exactly one line changed"; else bad "$changed diff lines, want 2 (one line out, one in)"; fi
for keep in '^    tag: sha-2222222$' 'tag: keep-me' 'tag: also-keep-me' '^tag: top-level-keep-me'; do
  if grep -q -- "$keep" "$work/values.yaml"; then ok "untouched: $keep"; else bad "lost: $keep"; fi
done

# 2. The other component is edited on its own.
run "$work/values.yaml" web sha-def5678
if [ "$rc" -eq 0 ] && [ "$out" = "changed: web.image.tag sha-2222222 -> sha-def5678" ]; then ok "web is bumped on its own"; else bad "web: rc=$rc out='$out'"; fi
if grep -q '^    tag: sha-abc1234$' "$work/values.yaml"; then ok "and the orchestrator's tag stays"; else bad "the web bump moved the orchestrator's tag"; fi

# 3. The same tag again is a no-op and leaves the file byte for byte alone.
cp "$work/values.yaml" "$work/after-first.yaml"
run "$work/values.yaml" web sha-def5678
case "$out" in unchanged*) unchanged=1 ;; *) unchanged=0 ;; esac
if [ "$rc" -eq 0 ] && [ "$unchanged" -eq 1 ]; then ok "the second run prints unchanged"; else bad "second run: rc=$rc out='$out'"; fi
if cmp -s "$work/after-first.yaml" "$work/values.yaml"; then ok "the second run leaves the file identical"; else bad "the second run modified the file"; fi

# 4. A quoted current tag is read correctly.
sed 's/^    tag: sha-2222222/    tag: "sha-3333333"/' "$work/before.yaml" > "$work/quoted.yaml"
run "$work/quoted.yaml" web sha-3333333
case "$out" in unchanged*) ok "a quoted current tag counts as current" ;; *) bad "quoted tag: rc=$rc out='$out'" ;; esac

# 5. Malformed tags and components are refused before the file is touched.
cp "$work/before.yaml" "$work/refuse.yaml"
for tag in latest sha-abc123 sha-abc12345 sha-ABCDEF1 sha-abcdefg v1.2.3 ""; do
  run "$work/refuse.yaml" web "$tag"
  if [ "$rc" -eq 2 ]; then ok "refuses tag '$tag' (exit 2)"; else bad "tag '$tag': rc=$rc, want 2"; fi
done
for component in chat oauth2Proxy edge "" "web:"; do
  run "$work/refuse.yaml" "$component" sha-abc1234
  if [ "$rc" -eq 2 ]; then ok "refuses component '$component' (exit 2)"; else bad "component '$component': rc=$rc, want 2"; fi
done
if cmp -s "$work/before.yaml" "$work/refuse.yaml"; then ok "refused input leaves the file alone"; else bad "refused input changed the file"; fi

# 6. Wrong argument count is a usage error.
run "$work/refuse.yaml" web
if [ "$rc" -eq 2 ]; then ok "two arguments are a usage error"; else bad "two arguments: rc=$rc"; fi
run
if [ "$rc" -eq 2 ]; then ok "no arguments is a usage error"; else bad "no arguments: rc=$rc"; fi

# 7. A values file without the component's tag fails without writing anything.
printf 'web:\n  image:\n    repository: x\norchestrator:\n  replicas: 1\n' > "$work/notag.yaml"
cp "$work/notag.yaml" "$work/notag.before"
run "$work/notag.yaml" web sha-abc1234
if [ "$rc" -eq 1 ]; then ok "no web.image.tag is an error (exit 1)"; else bad "no web.image.tag: rc=$rc, want 1"; fi
run "$work/notag.yaml" orchestrator sha-abc1234
if [ "$rc" -eq 1 ]; then ok "an orchestrator with no image block is an error (exit 1)"; else bad "no orchestrator.image.tag: rc=$rc, want 1"; fi
if cmp -s "$work/notag.before" "$work/notag.yaml"; then ok "the file without the tag is untouched"; else bad "it modified a file without the tag"; fi
run "$work/none.yaml" web sha-abc1234
if [ "$rc" -ne 0 ]; then ok "a missing file fails"; else bad "a missing file succeeded"; fi

# 8. The chart's real values.yaml (on a copy): bump both, then bump again.
cp "$here/../values.yaml" "$work/real.yaml"
run "$work/real.yaml" orchestrator sha-1234567
if [ "$rc" -eq 0 ] && grep -q '^    tag: sha-1234567' "$work/real.yaml"; then ok "the chart's values.yaml can be bumped (orchestrator)"; else bad "real values.yaml: rc=$rc out='$out'"; fi
run "$work/real.yaml" web sha-7654321
if [ "$rc" -eq 0 ] && grep -q '^    tag: sha-7654321' "$work/real.yaml"; then ok "the chart's values.yaml can be bumped (web)"; else bad "real values.yaml (web): rc=$rc out='$out'"; fi
run "$work/real.yaml" web sha-7654321
case "$out" in unchanged*) ok "and bumping it again is a no-op" ;; *) bad "real values.yaml, second run: '$out'" ;; esac
changed=$(diff "$here/../values.yaml" "$work/real.yaml" | grep -c '^[<>]' || true)
if [ "$changed" -eq 4 ]; then ok "the real file changed in exactly two lines"; else bad "real file: $changed diff lines, want 4"; fi
# Nothing but the two tags: the chat agent's tag, the third-party digests and the rest are not touched.
others=$(diff "$here/../values.yaml" "$work/real.yaml" | grep '^[<>]' | grep -vc '    tag: sha-' || true)
if [ "$others" -eq 0 ]; then ok "only tag lines differ"; else bad "$others other lines differ"; fi

if [ "$fail" -eq 0 ]; then echo "bump-tag tests passed"; else echo "bump-tag tests FAILED"; exit 1; fi
