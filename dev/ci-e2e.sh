#!/usr/bin/env sh
# System-level test of the CI gate (ADR 0017, ADR 0018): a gated job waits for a CI report, a red
# report sends the agent back, a green one for the new commit finishes the job. It plays CI with
# dev/ci-webhook.sh, so it exercises the whole path: signature, inbox, watch, gate.
#
#   dev/ci-e2e.sh                          # against the compose `edge` (http://127.0.0.1:8080)
#   BASE_URL=http://127.0.0.1:8080 AGENT_ID=mock-coder-ci dev/ci-e2e.sh
#
# It needs `mock-coder-ci` (dev/agents.yaml: the WireMock coder under `gate: {require: [ci]}` and
# `ci.required: [ci/build]`: only a report named ci/build counts), the
# webhook surface (ORCH_SURFACES=agui,webhook-generic and WEBHOOK_GENERIC_SECRETS in compose.yaml), and
# a Caddyfile that passes /webhooks/* on without an identity. It does not need the `app` profile's
# coder: only the default services and the orchestrator, edge and web of the profile.
#
# What it checks, in one run (a run is one POST /agui/agents/{agentId}, streamed until it ends):
#
#   1. a report with a wrong signature, and one with a stale timestamp, are 401 (nothing is stored);
#   2. the job waits: the mock says `red-once`, pushes commit 1111111 and completes, and the thread is
#      `verifying` with the gate `ci` and the sha of that commit;
#   3. a red report for that commit sends the agent back: attempt 2 (a new A2A task in the same
#      context), which pushes commit 2222222 and completes; the thread is `verifying` again;
#   4. a report for the OLD commit is a card and changes nothing (the thread stays `verifying`), and
#      neither does a `skipped` report of a check the gate does not name (`docs`): no first report
#      decides;
#   5. a green report for the new commit ends the job: `done`, attempt 2 of 3, and the run ends
#      RUN_FINISHED success;
#   6. the same report again (same timestamp and body, whatever the delivery id) is accepted twice
#      (202) and counted once;
#   7. the chat shows a `vymalo.ci` card for every report, each with an id of its own (conclusion,
#      short sha, link, summary).
#
# Environment (all optional):
#   BASE_URL    where the API and the webhook are served  (default http://127.0.0.1:8080, the compose `edge`)
#   AGENT_ID    the CI-gated agent                        (default mock-coder-ci)
#   AUTH_EMAIL  X-Auth-Request-Email to send              (default dev@example.com; the edge sets it anyway)
#   TIMEOUT     seconds any one wait may take             (default 90)
#
# The mock pushes the same two commits every time, and a commit is watched by the first job that pushed
# it, so the script passes once per database: start from a fresh stack (`docker compose down -v`) to run
# it again. (Real agents push new commits; ADR 0016, open question 30.)
#
# Exit status: 0 when every assertion holds, 1 otherwise. Needs: curl, jq, openssl (and /proc or
# uuidgen for a UUID).
set -eu

BASE_URL=${BASE_URL:-http://127.0.0.1:8080}
AGENT_ID=${AGENT_ID:-mock-coder-ci}
AUTH_EMAIL=${AUTH_EMAIL:-dev@example.com}
TIMEOUT=${TIMEOUT:-90}
here=$(cd "$(dirname "$0")" && pwd)
fail=0

# What the mock's `red-once` script reports (dev/wiremock/agent): the commit of attempt 1 and of
# attempt 2, in the repository dev/ci-webhook.sh reports on by default.
FIRST=1111111111111111111111111111111111111111
SECOND=2222222222222222222222222222222222222222

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

tmp=$(mktemp -d)
run_pid=
trap 'kill "$run_pid" 2>/dev/null || true; rm -rf "$tmp"' EXIT

expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then
    echo "ok    $1"
  else
    echo "FAIL  $1: expected '$3', got '$2'" >&2
    fail=1
  fi
}

api() { # api PATH: GET on the resource API
  curl -fsS -H "X-Auth-Request-Email: $AUTH_EMAIL" "$BASE_URL$1"
}

# ci ARGS...: play CI (dev/ci-webhook.sh) against the same base URL; prints its one status line.
ci() { BASE_URL=$BASE_URL sh "$here/ci-webhook.sh" "$@" | head -n 1; }

# wait_for DESCRIPTION JQ_EXPR EXPECTED: polls the thread until the jq expression over
# /api/threads/{id} gives EXPECTED, or TIMEOUT passes.
wait_for() {
  n=0
  limit=$((TIMEOUT * 5))
  while :; do
    got=$(api "/api/threads/$THREAD" 2>/dev/null | jq -r "$2" 2>/dev/null || true)
    [ "$got" != "$3" ] || { echo "ok    $1"; return 0; }
    n=$((n + 1))
    if [ "$n" -ge "$limit" ]; then
      echo "FAIL  $1: expected '$3', still '$got' after ${TIMEOUT}s" >&2
      echo "      (a stack that has run this script before watches these commits for the earlier job: docker compose down -v)" >&2
      fail=1
      return 1
    fi
    sleep 0.2
  done
}

echo "== the agent is there, gated on ci"
agents=$(api /api/agents | jq -r --arg id "$AGENT_ID" '[.[].id] | index($id) != null')
expect "GET /api/agents lists $AGENT_ID" "$agents" "true"

echo "== a delivery that is not signed by the secret is refused"
expect "a wrong secret is 401" "$(ci --sha "$FIRST" --secret not-the-secret --expect 401 | cut -d' ' -f1-2)" "HTTP 401"
expect "a stale timestamp is 401" \
  "$(ci --sha "$FIRST" --timestamp "$(($(date +%s) - 3600))" --expect 401 | cut -d' ' -f1-2)" "HTTP 401"

echo "== the job waits for CI"
THREAD=$(uuid)
input=$(jq -n --arg thread "$THREAD" --arg run "$(uuid)" --arg msg "$(uuid)" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: "red-once fix the login"}], forwardedProps: {}}')
# The run stays open while the job is queued, working or verifying, so it is left running.
curl -sS -N --max-time $((TIMEOUT * 4)) -o "$tmp/run.sse" -X POST "$BASE_URL/agui/agents/$AGENT_ID" \
  -H "X-Auth-Request-Email: $AUTH_EMAIL" -H 'content-type: application/json' -H 'accept: text/event-stream' \
  -d "$input" >/dev/null 2>&1 &
run_pid=$!
wait_for "attempt 1 pushed a commit and the thread is verifying" '[.state, .job.sha[0:7]] | join(" ")' "verifying 1111111" || exit 1
expect "the gate is ci, attempt 1 of 3" \
  "$(api "/api/threads/$THREAD" | jq -r '[(.job.gate | join("+")), .job.attempt, .job.maxAttempts] | join(" ")')" "ci 1 3"

echo "== a red report sends the agent back"
# The rework prompt quotes the report's summary as a finding, and the mock chooses its answer by the
# keywords in the prompt: the summary keeps `red-once` so the mock's second attempt pushes 2222222.
expect "the report is accepted" "$(ci --sha "$FIRST" --conclusion failure --branch agent/red-once \
  --summary 'red-once: tests::login fails: expected 200, got 500' | cut -d' ' -f1-2)" "HTTP 202"
wait_for "attempt 2 pushed commit 2222222 and the thread is verifying again" \
  '[.state, .job.attempt, .job.sha[0:7]] | join(" ")' "verifying 2 2222222" || exit 1

echo "== a report about the old commit changes nothing"
expect "the report is accepted" "$(ci --sha "$FIRST" --conclusion success | cut -d' ' -f1-2)" "HTTP 202"
sleep 2
expect "the thread is still verifying" "$(api "/api/threads/$THREAD" | jq -r '[.state, .job.attempt] | join(" ")')" "verifying 2"

echo "== a check the gate does not name cannot decide, even for the new commit"
expect "the report is accepted" "$(ci --sha "$SECOND" --name docs --conclusion skipped | cut -d' ' -f1-2)" "HTTP 202"
sleep 2
expect "the thread is still verifying" "$(api "/api/threads/$THREAD" | jq -r '[.state, .job.attempt] | join(" ")')" "verifying 2"

echo "== a green report for the new commit ends the job"
# The delivery id is not signed, so it is not what makes a report one report: the same timestamp and
# body are, whatever the id says.
at=$(date +%s)
expect "the report is accepted" "$(ci --sha "$SECOND" --conclusion success --branch agent/red-once --timestamp "$at" \
  --url 'https://ci.example.com/runs/2' --summary '212 tests passed' | cut -d' ' -f1-2)" "HTTP 202"
expect "the same report again, under another delivery id, is accepted too" \
  "$(ci --sha "$SECOND" --conclusion success --branch agent/red-once --timestamp "$at" \
    --url 'https://ci.example.com/runs/2' --summary '212 tests passed' --delivery "$(uuid)" | cut -d' ' -f1-2)" "HTTP 202"
wait_for "the thread is done at attempt 2 of 3" '[.state, .job.attempt, .job.maxAttempts] | join(" ")' "done 2 3" || exit 1

# The run ends when the job does.
n=0
while kill -0 "$run_pid" 2>/dev/null; do
  n=$((n + 1))
  [ "$n" -lt $((TIMEOUT * 5)) ] || break
  sleep 0.2
done
if kill -0 "$run_pid" 2>/dev/null; then
  echo "FAIL  the run did not end after the job was done" >&2
  fail=1
else
  run_pid=
  outcome=$(sed -n 's/^data: *//p' "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last | if . == null then "none" elif .type == "RUN_ERROR" then "error \(.code)" else "finished \(.outcome.type // "success")" end')
  expect "the run ended RUN_FINISHED (success)" "$outcome" "finished success"
fi

echo "== the chat shows every report as a card"
curl -sS --max-time 60 -H "X-Auth-Request-Email: $AUTH_EMAIL" -H 'accept: text/event-stream' \
  "$BASE_URL/agui/threads/$THREAD/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' >"$tmp/events.json" || echo '[]' >"$tmp/events.json"
expect "the vymalo.ci cards, in order: red, the old commit's report, the unnamed check, green" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ci") | "\(.content.shortSha)=\(.content.conclusion)"] | join(",")' "$tmp/events.json")" \
  "1111111=failure,1111111=success,2222222=skipped,2222222=success"
expect "every report has a card of its own: the later report of 1111111 did not replace the red one" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ci") | .messageId] | unique | length' "$tmp/events.json")" "4"
expect "no card replaces another" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ci") | .replace] | unique | join(",")' "$tmp/events.json")" "false"
expect "the card carries the link and the summary" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ci" and .content.shortSha == "2222222" and .content.name == "ci/build")][0].content | [.passed, .url, .summary] | join(" ")' "$tmp/events.json")" \
  "true https://ci.example.com/runs/2 212 tests passed"

[ "$fail" -eq 0 ] && echo "all checks passed"
exit "$fail"
