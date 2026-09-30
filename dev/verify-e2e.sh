#!/usr/bin/env sh
# Drives the verification gate (ADR 0018) of a running stack over AG-UI, the way the web does, and
# asserts what a user would see. It needs `mock-coder-gated` (dev/agents.yaml: the WireMock coder
# under `gate: {require: [agent-checks]}`) and its scenarios `red-once` and `red-always`
# (dev/wiremock/agent, checked by dev/check-mocks.sh).
#
#   dev/verify-e2e.sh                     # against the compose `edge` (http://127.0.0.1:8080)
#   BASE_URL=http://127.0.0.1:8080 AGENT_ID=mock-coder-gated dev/verify-e2e.sh
#
# What it checks, one run each (a run is one POST /agui/agents/{agentId}, streamed until it ends):
#
#   1. `red-once`: the agent's own checks fail, the orchestrator sends the agent back with the
#      findings (a new A2A task in the same context), the second attempt passes. One run, two
#      subagents, a `vymalo.check` that failed and one that passed, a `vymalo.rework`, and the run
#      ends `RUN_FINISHED` success with `job.attempt` 2 in the final `STATE_SNAPSHOT`; the thread
#      of the resource API is `done` and carries the same job, and its export (GET /api/threads/{id}/export)
#      holds the whole job with the commit that passed and the rework in the log.
#   2. `red-always`: three attempts (the default), then `RUN_ERROR` with `code: "checks_failed"`;
#      the thread is `failed`.
#   3. A run may lower the attempts in `forwardedProps["vymalo.gate"]`: `maxAttempts: 2` ends after two.
#   4. A run may not weaken the gate or ask for more than the deployment can honour: removing
#      the source, more attempts than ORCH_MAX_ATTEMPTS_CAP, `ci` where no check is named
#      (`ci.required`) and `verifier` where no verifier agent is configured are each a 400 problem
#      before any stream, and no thread is created.
#
# Environment (all optional):
#   BASE_URL    where the API is served     (default http://127.0.0.1:8080, the compose `edge`)
#   AGENT_ID    the gated agent             (default mock-coder-gated)
#   AUTH_EMAIL  X-Auth-Request-Email to send (default dev@example.com; the compose `edge` sets it anyway)
#   TIMEOUT     seconds a run may take      (default 90)
#
# Exit status: 0 when every assertion holds, 1 otherwise. Needs: curl, jq (and /proc or uuidgen).
set -eu

BASE_URL=${BASE_URL:-http://127.0.0.1:8080}
AGENT_ID=${AGENT_ID:-mock-coder-gated}
AUTH_EMAIL=${AUTH_EMAIL:-dev@example.com}
TIMEOUT=${TIMEOUT:-90}
fail=0

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

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

# run_input THREAD TEXT [GATE_JSON]: a RunAgentInput, with the gate request in forwardedProps when given.
run_input() {
  jq -n --arg thread "$1" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" --arg gate "${3:-}" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}],
    forwardedProps: (if $gate == "" then {} else {"vymalo.gate": ($gate | fromjson)} end)
  }'
}

# run TEXT [GATE_JSON]: starts a run on a new thread and waits for its response to end. Sets THREAD and
# writes the AG-UI events of the response, an array, to $tmp/events.json.
run() {
  THREAD=$(uuid)
  code=$(curl -sS -N --max-time "$TIMEOUT" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$BASE_URL/agui/agents/$AGENT_ID" -H "X-Auth-Request-Email: $AUTH_EMAIL" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' \
    -d "$(run_input "$THREAD" "$1" "${2:-}")" || true)
  if [ "$code" != 200 ]; then
    echo "FAIL  POST /agui/agents/$AGENT_ID answered HTTP ${code:-none}: $(head -c 400 "$tmp/run.sse" 2>/dev/null)" >&2
    exit 1
  fi
  sed -n 's/^data: *//p' "$tmp/run.sse" | jq -s '.' >"$tmp/events.json"
}

# ev JQ: a jq expression over the events of the last run, as raw text.
ev() { jq -r "$1" "$tmp/events.json"; }

wait_state() { # wait_state THREAD STATE: the resource API's view catches up with the stream
  n=0
  while [ "$(api "/api/threads/$1" | jq -r .state)" != "$2" ]; do
    n=$((n + 1))
    [ "$n" -lt 100 ] || { echo "FAIL  thread $1 never reached $2" >&2; fail=1; return; }
    sleep 0.2
  done
}

echo "== red-once: red, sent back, green"
run 'red-once fix the login'
expect "one run, ended once" "$(ev '[.[] | select(.type == "RUN_STARTED")] | length'),$(ev '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | length')" "1,1"
expect "it ends in success" "$(ev '.[-1] | [.type, .outcome.type] | join(" ")')" "RUN_FINISHED success"
expect "the agent ran twice: two subagents, each finished" \
  "$(ev '[([.[] | select(.type == "SUBAGENT_STARTED")] | length), ([.[] | select(.type == "SUBAGENT_FINISHED")] | length)] | join(",")')" "2,2"
expect "the checks, in order: failed at attempt 1, passed at attempt 2" \
  "$(ev '[.[] | select(.activityType == "vymalo.check") | "\(.content.attempt)=\(.content.status)"] | join(",")')" "1=failed,2=passed"
expect "the failed check says what is wrong" \
  "$(ev '[.[] | select(.activityType == "vymalo.check" and .content.status == "failed")][0].content.findings[0]')" \
  "red-once: tests::login fails: expected 200, got 500"
expect "one rework, into attempt 2 of 3" \
  "$(ev '[.[] | select(.activityType == "vymalo.rework") | "\(.content.attempt)/\(.content.maxAttempts)"] | join(",")')" "2/3"
expect "the job at the end: done, attempt 2 of 3, the gate, the second commit" \
  "$(ev '[.[] | select(.type == "STATE_SNAPSHOT")][-1] | [.snapshot.thread.state, .snapshot.job.attempt, .snapshot.job.maxAttempts, (.snapshot.job.gate | join("+")), .snapshot.job.sha[0:7]] | join(" ")')" \
  "done 2 3 agent_checks 2222222"
wait_state "$THREAD" "done"
expect "the thread of the resource API: done, and the job with it" \
  "$(api "/api/threads/$THREAD" | jq -r '[.state, .job.attempt, .job.maxAttempts, (.job.gate | join("+"))] | join(" ")')" "done 2 3 agent_checks"
expect "its export (dev/export-thread.sh): the whole job, the commit that passed, and a log with the rework in it" \
  "$(api "/api/threads/$THREAD/export" | jq -r '[.format, .version, .job.attempt, .job.pushed.commit[0:7], (.events | length > 0), ([.events[] | select(.kind == "rework")] | length)] | join(" ")')" \
  "another-agentic-system/thread-export 1 2 2222222 true 1"

echo "== red-always: three attempts, then checks_failed"
run 'red-always fix the login'
expect "it ends in RUN_ERROR checks_failed" "$(ev '.[-1] | [.type, .code] | join(" ")')" "RUN_ERROR checks_failed"
expect "one check per attempt, all failed" \
  "$(ev '[.[] | select(.activityType == "vymalo.check") | "\(.content.attempt)=\(.content.status)"] | join(",")')" "1=failed,2=failed,3=failed"
expect "two reworks: into attempts 2 and 3" \
  "$(ev '[.[] | select(.activityType == "vymalo.rework") | .content.attempt] | join(",")')" "2,3"
expect "the last state: failed at attempt 3 of 3" \
  "$(ev '[.[] | select(.type == "STATE_SNAPSHOT")][-1] | [.snapshot.thread.state, .snapshot.job.attempt, .snapshot.job.maxAttempts] | join(" ")')" "failed 3 3"
wait_state "$THREAD" failed
expect "the thread of the resource API: failed, at attempt 3" \
  "$(api "/api/threads/$THREAD" | jq -r '[.state, .job.attempt] | join(" ")')" "failed 3"

echo "== a run may lower the attempts"
run 'red-always fix the login' '{"maxAttempts": 2}'
expect "it fails after two attempts" \
  "$(ev '[(.[-1] | .code), ([.[] | select(.activityType == "vymalo.check")] | length)] | join(" ")')" "checks_failed 2"

echo "== a run may not weaken the gate"
refused() { # refused DESCRIPTION GATE_JSON EXPECTED_DETAIL_FRAGMENT
  thread=$(uuid)
  code=$(curl -sS -o "$tmp/problem.json" -w '%{http_code}' -X POST "$BASE_URL/agui/agents/$AGENT_ID" \
    -H "X-Auth-Request-Email: $AUTH_EMAIL" -H 'content-type: application/json' -H 'accept: text/event-stream' \
    -d "$(run_input "$thread" 'red-once please' "$2")")
  expect "$1: 400" "$code" "400"
  expect "$1: the problem says why" \
    "$(jq -r --arg f "$3" '.detail | contains($f)' "$tmp/problem.json" 2>/dev/null || echo unreadable)" "true"
  expect "$1: no thread was created" \
    "$(curl -s -o /dev/null -w '%{http_code}' -H "X-Auth-Request-Email: $AUTH_EMAIL" "$BASE_URL/api/threads/$thread")" "404"
}
refused "removing the source" '{"require": []}' "may add sources"
refused "more attempts than the cap" '{"maxAttempts": 99}' "maxAttempts"
refused "ci with no check named" '{"require": ["agent-checks", "ci"]}' "no check is named"
refused "the verifier with no verifier agent" '{"require": ["agent-checks", "verifier"]}' "no verifier agent is configured"

[ "$fail" -eq 0 ] && echo "all checks passed"
exit "$fail"
