#!/usr/bin/env sh
# Signs and posts one CI report to the orchestrator's webhook, the way a CI system would
# (docs/api/webhooks.md, ADR 0017). Use it to play CI by hand, or from dev/ci-e2e.sh.
#
#   dev/ci-webhook.sh --sha 1111111111111111111111111111111111111111 --conclusion failure
#   dev/ci-webhook.sh --repo https://github.com/example/sandbox.git --sha <40 hex> --name ci/build \
#                     --conclusion success --branch agent/red-once --summary '212 tests passed'
#   dev/ci-webhook.sh --sha <40 hex> --secret wrong --expect 401     # a bad signature is refused
#
# Options (all optional except --sha):
#   --shape SHAPE       generic (POST /webhooks/ci, the default)
#   --sha SHA           the commit the check ran on: 40 hex characters
#   --repo URL          the repository (default https://github.com/example/sandbox.git, the one the
#                       mock agent reports in its `branch` artifact)
#   --name NAME         the check's name (default ci/build)
#   --conclusion C      success (default), neutral, skipped, failure, cancelled, timed_out,
#                       action_required, stale
#   --branch BRANCH     the branch, for display (default: none)
#   --url URL           a link to the run, shown on the card (default: none)
#   --summary TEXT      a short text, shown on the card and quoted, as untrusted data, in a rework
#                       prompt (default: none)
#   --delivery ID       the delivery id, a UUID: the same id twice is one report (default: a new one)
#   --secret SECRET     the shared secret (default $WEBHOOK_SECRET, else dev-webhook-secret, the value
#                       of WEBHOOK_GENERIC_SECRETS in compose.yaml)
#   --timestamp SECONDS the Unix time to sign (default: now); an old one is refused with 401
#   --base URL          where the orchestrator is served (default $BASE_URL, else
#                       http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`, which passes
#                       /webhooks/* on without an identity)
#   --expect CODE       the status that counts as success (default 202); anything else exits 1
#
# Prints the HTTP status and the body. Needs curl, jq and openssl (and /proc or uuidgen for a UUID).
set -eu

shape=generic
sha=
repo=https://github.com/example/sandbox.git
name=ci/build
conclusion=success
branch=
link=
summary=
delivery=
secret=${WEBHOOK_SECRET:-dev-webhook-secret}
timestamp=
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
expect=202

usage() { sed -n '2,/^set -eu/p' "$0" | sed -n 's/^# \{0,1\}//p' | sed '$d'; }
die() { echo "ci-webhook.sh: $*" >&2; exit 2; }

while [ $# -gt 0 ]; do
  case $1 in
    -h | --help) usage; exit 0 ;;
    --shape | --sha | --repo | --name | --conclusion | --branch | --url | --summary | --delivery | \
      --secret | --timestamp | --base | --expect)
      [ $# -ge 2 ] || die "$1 needs a value"
      case $1 in
        --shape) shape=$2 ;;
        --sha) sha=$2 ;;
        --repo) repo=$2 ;;
        --name) name=$2 ;;
        --conclusion) conclusion=$2 ;;
        --branch) branch=$2 ;;
        --url) link=$2 ;;
        --summary) summary=$2 ;;
        --delivery) delivery=$2 ;;
        --secret) secret=$2 ;;
        --timestamp) timestamp=$2 ;;
        --base) base=$2 ;;
        --expect) expect=$2 ;;
      esac
      shift 2
      ;;
    *) die "unknown option $1 (--help lists them)" ;;
  esac
done
base=${base%/}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }
[ -n "$sha" ] || die "--sha is required"
[ -n "$delivery" ] || delivery=$(uuid)

# hmac SECRET: HMAC-SHA-256 of stdin as lowercase hex.
hmac() { openssl dgst -sha256 -hmac "$1" -r | cut -d' ' -f1; }

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

case $shape in
  generic)
    body=$(jq -cn --arg repo "$repo" --arg sha "$sha" --arg name "$name" --arg conclusion "$conclusion" \
      --arg branch "$branch" --arg url "$link" --arg summary "$summary" '{
        version: 1, repository: $repo, sha: $sha, name: $name, conclusion: $conclusion}
        + (if $branch == "" then {} else {branch: $branch} end)
        + (if $url == "" then {} else {url: $url} end)
        + (if $summary == "" then {} else {summary: $summary} end)')
    # The signature covers "<timestamp>.<body>", the timestamp as sent, then a full stop, then the
    # exact body bytes. `printf` adds no newline, and --data-binary sends none either.
    [ -n "$timestamp" ] || timestamp=$(date +%s)
    signature=$(printf '%s.%s' "$timestamp" "$body" | hmac "$secret")
    status=$(curl -sS -o "$tmp" -w '%{http_code}' --max-time 30 -X POST "$base/webhooks/ci" \
      -H 'Content-Type: application/json' \
      -H "X-Vymalo-Delivery: $delivery" \
      -H "X-Vymalo-Timestamp: $timestamp" \
      -H "X-Vymalo-Signature-256: sha256=$signature" \
      --data-binary "$body") || die "cannot reach $base/webhooks/ci"
    ;;
  *) die "unknown --shape '$shape' (generic)" ;;
esac

echo "HTTP $status ($shape, delivery $delivery, $conclusion $name on $(printf '%s' "$sha" | cut -c1-7))"
if [ -s "$tmp" ]; then cat "$tmp"; echo; fi
[ "$status" = "$expect" ] || { echo "ci-webhook.sh: expected HTTP $expect" >&2; exit 1; }
