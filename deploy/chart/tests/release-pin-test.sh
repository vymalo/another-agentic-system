#!/bin/sh
# Tests of deploy/chart/release-pin.sh: it pins only `orchestrator.image` and `web.image` (tag and digest), only in a copy, and
# refuses input it does not understand; and the chart renders the pinned copy with `repository:X.Y.Z@sha256:...`. Needs sh, awk,
# cmp, diff, git and helm.
#
#   sh deploy/chart/tests/release-pin-test.sh        (from the repository root)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
chart=$(cd "$here/.." && pwd)
pin="$chart/release-pin.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0

ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

# run <args...>: sets $out and $rc without aborting the script.
run() {
  rc=0
  out=$(sh "$pin" "$@" 2>&1) || rc=$?
}

od=sha256:1111111111111111111111111111111111111111111111111111111111111111
wd=sha256:2222222222222222222222222222222222222222222222222222222222222222

cat > "$work/values.yaml" <<'YAML'
# A comment mentioning orchestrator: and tag: must not confuse the edit.
host: ""
orchestrator:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/orchestrator
    tag: sha-1111111 # bumped by CI
    pullPolicy: IfNotPresent
  artifacts:
    image:
      tag: keep-me-nested
  surfaces: [agui]
web:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/web
    tag: sha-2222222
    digest: sha256:0000000000000000000000000000000000000000000000000000000000000000
  replicas: 1
webSearch:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/searxng-mcp
    tag: sha-4444444
chat:
  image:
    tag: keep-me
    digest: sha256:0
tag: top-level-keep-me
YAML

# 1. The two tags become the version, each followed by its digest; a digest that was there is replaced; nothing else moves.
cp "$work/values.yaml" "$work/before.yaml"
run "$work/values.yaml" 1.2.3 "$od" "$wd"
cat > "$work/want.yaml" <<YAML
# A comment mentioning orchestrator: and tag: must not confuse the edit.
host: ""
orchestrator:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/orchestrator
    tag: "1.2.3"
    digest: "$od"
    pullPolicy: IfNotPresent
  artifacts:
    image:
      tag: keep-me-nested
  surfaces: [agui]
web:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/web
    tag: "1.2.3"
    digest: "$wd"
  replicas: 1
webSearch:
  image:
    repository: ghcr.io/vymalo/another-agentic-system/searxng-mcp
    tag: sha-4444444
chat:
  image:
    tag: keep-me
    digest: sha256:0
tag: top-level-keep-me
YAML
if [ "$rc" -eq 0 ] && cmp -s "$work/values.yaml" "$work/want.yaml"; then ok "pins the two tags and digests and nothing else"; else bad "pin: rc=$rc $out"; diff "$work/want.yaml" "$work/values.yaml" || true; fi

# 2. A second run gives the same file (a re-run of a release packages the same values).
cp "$work/values.yaml" "$work/once.yaml"
run "$work/values.yaml" 1.2.3 "$od" "$wd"
if [ "$rc" -eq 0 ] && cmp -s "$work/values.yaml" "$work/once.yaml"; then ok "a second run changes nothing"; else bad "second run: rc=$rc $out"; fi

# 3. Refusals: a version with a v, a moving X.Y, a short digest, a missing file, a block that is not there; each leaves the file.
refuses() { # refuses <want rc> <description> <args...>
  want=$1; desc=$2; shift 2
  cp "$work/before.yaml" "$work/r.yaml"
  run "$@"
  if [ "$rc" -eq "$want" ] && cmp -s "$work/r.yaml" "$work/before.yaml"; then ok "refuses $desc"; else bad "$desc: rc=$rc (want $want) $out"; fi
}
refuses 2 "a version with a v" "$work/r.yaml" v1.2.3 "$od" "$wd"
refuses 2 "a moving X.Y" "$work/r.yaml" 1.2 "$od" "$wd"
refuses 2 "a short digest" "$work/r.yaml" 1.2.3 sha256:abc "$wd"
refuses 2 "a digest without sha256:" "$work/r.yaml" 1.2.3 "$od" 2222222222222222222222222222222222222222222222222222222222222222
refuses 2 "a missing file" "$work/nope.yaml" 1.2.3 "$od" "$wd"
refuses 2 "three arguments" "$work/r.yaml" 1.2.3 "$od"
grep -v '^web:' "$work/before.yaml" | sed 's/^  image:$/  img:/' > "$work/noweb.yaml"
cp "$work/noweb.yaml" "$work/noweb.before.yaml"
run "$work/noweb.yaml" 1.2.3 "$od" "$wd"
if [ "$rc" -eq 1 ] && cmp -s "$work/noweb.yaml" "$work/noweb.before.yaml"; then ok "refuses a file without both image blocks, and leaves it"; else bad "no web block: rc=$rc $out"; fi

# 4. The chart's own values.yaml is tracked by git: refused, untouched.
before=$(cksum < "$chart/values.yaml")
run "$chart/values.yaml" 1.2.3 "$od" "$wd"
if [ "$rc" -eq 2 ] && [ "$(cksum < "$chart/values.yaml")" = "$before" ]; then ok "refuses the values.yaml git tracks"; else bad "tracked file: rc=$rc $out"; fi

# 5. A copy of the chart, pinned, renders both images by version and digest and no sha-<7> for them; the copy from git does not
#    change. The render needs a host and an issuer: the netcup example, and the test-only app-link identities deploy.yml layers.
cp -R "$chart" "$work/chart"
rm -rf "$work/chart/tests"
run "$work/chart/values.yaml" 1.2.3 "$od" "$wd"
if [ "$rc" -eq 0 ]; then ok "pins a copy of the chart"; else bad "copy: rc=$rc $out"; fi
if helm template another-agentic-system "$work/chart" --namespace another-agentic-system \
  -f "$chart/examples/netcup.values.yaml" -f "$chart/tests/mobile-app-links.values.yaml" > "$work/rendered.yaml" 2> "$work/err"; then
  ok "the pinned copy renders"
else
  bad "the pinned copy does not render: $(cat "$work/err")"
fi
if grep -Fqx "          image: \"ghcr.io/vymalo/another-agentic-system/orchestrator:1.2.3@$od\"" "$work/rendered.yaml"; then ok "the orchestrator runs X.Y.Z@digest"; else bad "orchestrator image line"; fi
if grep -Fqx "          image: \"ghcr.io/vymalo/another-agentic-system/web:1.2.3@$wd\"" "$work/rendered.yaml"; then ok "the web runs X.Y.Z@digest"; else bad "web image line"; fi
if grep -Eq "another-agentic-system/(orchestrator|web):sha-[0-9a-f]{7}" "$work/rendered.yaml"; then bad "a sha-<7> tag is left for the orchestrator or the web"; else ok "no sha-<7> tag is left for them"; fi
if git -C "$chart" diff --quiet -- values.yaml; then ok "the chart in git is unchanged"; else bad "the chart's values.yaml changed"; fi

exit "$fail"
