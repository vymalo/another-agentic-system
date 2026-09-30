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
#   --shape SHAPE       generic (POST /webhooks/ci, the default), or github (POST /webhooks/github: a
#                       GitHub delivery, signed over the raw body with X-Hub-Signature-256)
#   --event EVENT       github only: check_run (default) or workflow_run, always with action completed.
#                       The check's name is the run's name or the workflow's name (a `check_suite` is
#                       not reported: it names an app, not a check); only check_run carries a summary
#   --run-id N          github only: the id of the check run or workflow run (default: a new one). The
#                       same id and completion time twice is one report
#   --completed-at T    github only: the completion time, ISO 8601 UTC (default: now); an event older
#                       than WEBHOOK_GITHUB_MAX_AGE_SECS (a day) is acknowledged and not stored
#   --fork              github only: the run is of a fork's code (head repository differs): acknowledged
#                       (202) and not stored
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
#   --delivery ID       the delivery id, for the logs only: it is not signed, so it decides nothing (the
#                       generic report is one delivery by its timestamp and body) (default: a new one)
#   --secret SECRET     the shared secret (default $WEBHOOK_SECRET, else the value of
#                       WEBHOOK_GENERIC_SECRETS and WEBHOOK_GITHUB_SECRETS in compose.yaml)
#   --timestamp SECONDS generic only: the Unix time to sign (default: now); an old one is refused with 401.
#                       The same timestamp and body twice is one report
#   --base URL          where the orchestrator is served (default $BASE_URL, else
#                       http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`, which passes
#                       /webhooks/* on without an identity)
#   --expect CODE       the status that counts as success (default 202; 204 for a github ping);
#                       anything else exits 1
#   --ping              github only: send the `ping` event GitHub sends when a webhook is created
#
# Prints the HTTP status and the body. Needs curl, jq and openssl (and /proc or uuidgen for a UUID).
set -eu

shape=generic
event=check_run
run_id=
completed_at=
fork=
ping=
sha=
repo=https://github.com/example/sandbox.git
name=ci/build
conclusion=success
branch=
link=
summary=
delivery=
secret=${WEBHOOK_SECRET:-dev-webhook-secret-0123456789abcdef0123}
timestamp=
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
expect=202

usage() { sed -n '2,/^set -eu/p' "$0" | sed -n 's/^# \{0,1\}//p' | sed '$d'; }
die() { echo "ci-webhook.sh: $*" >&2; exit 2; }

while [ $# -gt 0 ]; do
  case $1 in
    -h | --help) usage; exit 0 ;;
    --ping) ping=1; shift ;;
    --fork) fork=1; shift ;;
    --shape | --event | --sha | --repo | --name | --conclusion | --branch | --url | --summary | --delivery | \
      --secret | --timestamp | --base | --expect | --run-id | --completed-at)
      [ $# -ge 2 ] || die "$1 needs a value"
      case $1 in
        --shape) shape=$2 ;;
        --event) event=$2 ;;
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
        --run-id) run_id=$2 ;;
        --completed-at) completed_at=$2 ;;
      esac
      shift 2
      ;;
    *) die "unknown option $1 (--help lists them)" ;;
  esac
done
base=${base%/}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }
[ -n "$sha" ] || [ -n "$ping" ] || die "--sha is required"
[ -n "$delivery" ] || delivery=$(uuid)
[ -n "$run_id" ] || run_id=$(($(date +%s) * 1000 + $$ % 1000))
[ -n "$completed_at" ] || completed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)

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
  github)
    if [ -n "$ping" ]; then
      event=ping
      expect_default=204
      body=$(jq -cn '{zen: "Keep it logically awesome.", hook_id: 1, hook: {type: "Repository", id: 1, active: true, events: ["check_run", "workflow_run"]}}')
    else
      expect_default=202
      # The repository's web address: the repository without `.git`.
      html=${repo%.git}
      # The head repository of a workflow run is the repository, or (--fork) another one.
      head_id=1
      [ -z "$fork" ] || head_id=2
      body=$(jq -cn --arg event "$event" --arg html "$html" --arg sha "$sha" --arg name "$name" \
        --arg conclusion "$conclusion" --arg branch "$branch" --arg url "$link" --arg summary "$summary" \
        --argjson id "$run_id" --arg at "$completed_at" --argjson head "$head_id" '
        def opt(k; v): if v == "" then {} else {(k): v} end;
        def repo(i): {id: i, full_name: ($html | sub("^https?://[^/]+/"; "")), html_url: $html};
        {action: "completed", repository: repo(1)} +
        (if $event == "check_run" then
           {check_run: ({id: $id, head_sha: $sha, name: $name, status: "completed", conclusion: $conclusion,
                         completed_at: $at,
                         check_suite: (if $branch == "" then {head_branch: null} else {head_branch: $branch} end),
                         pull_requests: (if $head == 1 then [] else [{head: {repo: {id: $head}}}] end),
                         output: ({title: $name} + opt("summary"; $summary))} + opt("html_url"; $url))}
         elif $event == "workflow_run" then
           {workflow_run: ({id: $id, head_sha: $sha, name: $name, status: "completed", conclusion: $conclusion,
                            run_attempt: 1, updated_at: $at, head_repository: repo($head)}
                           + (if $branch == "" then {head_branch: null} else {head_branch: $branch} end) + opt("html_url"; $url))}
         else error("unknown --event " + $event) end)') || die "--event must be check_run or workflow_run"
    fi
    # GitHub signs the raw body alone (no timestamp) and names the event and the delivery.
    [ "$expect" != 202 ] || expect=$expect_default
    status=$(curl -sS -o "$tmp" -w '%{http_code}' --max-time 30 -X POST "$base/webhooks/github" \
      -H 'Content-Type: application/json' -H 'User-Agent: GitHub-Hookshot/dev' \
      -H "X-GitHub-Event: $event" -H "X-GitHub-Delivery: $delivery" \
      -H "X-Hub-Signature-256: sha256=$(printf '%s' "$body" | hmac "$secret")" \
      --data-binary "$body") || die "cannot reach $base/webhooks/github"
    ;;
  *) die "unknown --shape '$shape' (generic or github)" ;;
esac

if [ -n "$ping" ]; then what=ping; else what="$conclusion $name on $(printf '%s' "$sha" | cut -c1-7)"; fi
echo "HTTP $status ($shape, delivery $delivery, $what)"
if [ -s "$tmp" ]; then cat "$tmp"; echo; fi
[ "$status" = "$expect" ] || { echo "ci-webhook.sh: expected HTTP $expect" >&2; exit 1; }
