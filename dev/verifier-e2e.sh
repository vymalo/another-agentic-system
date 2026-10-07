#!/usr/bin/env sh
# Drives the verifier agent of the gate (ADR 0018) in a running stack over AG-UI, the way the web
# does, and asserts what a user would see. It needs `mock-coder-verified` (dev/agents.yaml: the
# WireMock coder under `gate: {require: [verifier], verifier: verifier}`), the WireMock verifier
# behind `verifier` (dev/wiremock/verifier) and the coder's keywords `push-flawed` and `push-clean`
# (dev/wiremock/agent), all checked by dev/check-mocks.sh.
#
#   dev/verifier-e2e.sh                     # against the compose `edge` (http://127.0.0.1:8080)
#   BASE_URL=http://127.0.0.1:8080 AGENT_ID=mock-coder-verified dev/verifier-e2e.sh
#
# What it checks, one run each (a run is one POST /agui/agents/{agentId}, streamed until it ends):
#
#   1. `push-flawed`: the coder pushes commit `aaaa…`, the verifier answers with findings, the
#      orchestrator sends the coder back with them (a new A2A task in the same context), the coder
#      pushes `bbbb…` and the verifier passes it. One run, four subagents (the coder twice, the
#      verifier twice, named after it), a `vymalo.check` of the verifier that failed and one that
#      passed, a `vymalo.rework`, and `RUN_FINISHED` success with `job.attempt` 2 and the gate
#      `verifier` in the final `STATE_SNAPSHOT`; the thread of the resource API is `done`.
#   2. `push-flawed` with `maxAttempts: 1`: the findings are final, `RUN_ERROR` `checks_failed`,
#      and the thread is `failed`.
#   3. `push-clean`: the verifier passes the first commit, no rework, attempt 1.
#   4. What the verifier was sent, read from the mock's own request journal: no context (the verifier
#      starts a conversation of its own, once per verification, ADR 0055: the thread's id is never
#      named), the commit, and the task quoted as untrusted data (skipped when the mock's admin API
#      is not reachable). The requests of the first run are found by a marker in its task.
#   5. A run may not choose the verifier, nor drop it: each is a 400 problem before any stream,
#      and no thread is created.
#
# Environment (all optional):
#   BASE_URL      where the API is served          (default http://127.0.0.1:8080, the compose `edge`)
#   AGENT_ID      the verified agent               (default mock-coder-verified)
#   VERIFIER_URL  the mock verifier's own port     (default http://127.0.0.1:8083; its journal is read there)
#   AUTH_EMAIL    the user (default dev@example.com): a token of the mock issuer (dev/auth-header.sh)
#   TIMEOUT       seconds a run may take           (default 90)
#
# Exit status: 0 when every assertion holds, 1 otherwise. Needs: curl, jq (and /proc or uuidgen).
set -eu

BASE_URL=${BASE_URL:-http://127.0.0.1:8080}
AGENT_ID=${AGENT_ID:-mock-coder-verified}
VERIFIER_URL=${VERIFIER_URL:-http://127.0.0.1:8083}
AUTH_EMAIL=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$AUTH_EMAIL")
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
  curl -fsS -H "$id_header" "$BASE_URL$1"
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
    "$BASE_URL/agui/agents/$AGENT_ID" -H "$id_header" \
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

echo "== push-flawed: findings, sent back, passed"
# The task carries a marker of its own: the verifier's requests name no context (ADR 0055), so the journal's
# requests of this run are the ones that quote it.
MARKER="run-$(uuid)"
run "push-flawed fix the login ($MARKER)"
FLAWED=$THREAD
expect "one run, ended once" "$(ev '[.[] | select(.type == "RUN_STARTED")] | length'),$(ev '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | length')" "1,1"
expect "it ends in success" "$(ev '.[-1] | [.type, .outcome.type] | join(" ")')" "RUN_FINISHED success"
expect "the coder and the verifier each ran twice, as subagents of their own" \
  "$(ev '[.[] | select(.type == "SUBAGENT_STARTED") | .name] | join(",")')" \
  "mock-coder-verified,verifier,mock-coder-verified,verifier"
expect "every subagent that started ended" \
  "$(ev '[([.[] | select(.type == "SUBAGENT_STARTED")] | length), ([.[] | select(.type == "SUBAGENT_FINISHED")] | length)] | join(",")')" "4,4"
expect "the verifier's verdicts, in order: findings, then a pass" \
  "$(ev '[.[] | select(.type == "SUBAGENT_FINISHED" and .result.passed != null) | .result.passed] | map(tostring) | join(",")')" "false,true"
expect "the verifier's cards, in order: pending and failed at attempt 1, pending and passed at attempt 2" \
  "$(ev '[.[] | select(.activityType == "vymalo.check") | "\(.content.source)@\(.content.attempt)=\(.content.status)"] | join(",")')" \
  "verifier@1=pending,verifier@1=failed,verifier@2=pending,verifier@2=passed"
expect "the failed card carries the verifier's finding" \
  "$(ev '[.[] | select(.activityType == "vymalo.check" and .content.status == "failed")][0].content.findings[0]')" \
  "src/login.rs: the empty password is accepted; add a test that covers it"
expect "one rework, into attempt 2 of 3" \
  "$(ev '[.[] | select(.activityType == "vymalo.rework") | "\(.content.attempt)/\(.content.maxAttempts)"] | join(",")')" "2/3"
expect "the job at the end: done, attempt 2 of 3, the gate, the second commit" \
  "$(ev '[.[] | select(.type == "STATE_SNAPSHOT")][-1] | [.snapshot.thread.state, .snapshot.job.attempt, .snapshot.job.maxAttempts, (.snapshot.job.gate | join("+")), .snapshot.job.sha[0:7]] | join(" ")')" \
  "done 2 3 verifier bbbbbbb"
wait_state "$FLAWED" "done"
expect "the thread of the resource API: done, and the job with it" \
  "$(api "/api/threads/$FLAWED" | jq -r '[.state, .job.attempt, .job.maxAttempts, (.job.gate | join("+"))] | join(" ")')" "done 2 3 verifier"

echo "== push-flawed with one attempt: the findings are final"
run 'push-flawed fix the login' '{"maxAttempts": 1}'
expect "it ends in RUN_ERROR checks_failed" "$(ev '.[-1] | [.type, .code] | join(" ")')" "RUN_ERROR checks_failed"
expect "the verifier ran once and its card failed" \
  "$(ev '[.[] | select(.activityType == "vymalo.check" and .content.status != "pending") | .content.status] | join(",")')" "failed"
expect "the error says what the verifier found" \
  "$(ev '.[-1].message | contains("the empty password is accepted")')" "true"
wait_state "$THREAD" failed

echo "== push-clean: passed at once"
run 'push-clean fix the login'
CLEAN=$THREAD
expect "it ends in success, without a rework" \
  "$(ev '[(.[-1] | .outcome.type), ([.[] | select(.activityType == "vymalo.rework")] | length)] | map(tostring) | join(" ")')" "success 0"
expect "the job at the end: done, attempt 1, the clean commit" \
  "$(ev '[.[] | select(.type == "STATE_SNAPSHOT")][-1] | [.snapshot.thread.state, .snapshot.job.attempt, .snapshot.job.sha[0:7]] | join(" ")')" "done 1 ccccccc"
wait_state "$CLEAN" "done"

echo "== what the verifier was sent"
journal=$(curl -fsS -m 5 "$VERIFIER_URL/__admin/requests" 2>/dev/null || true)
if [ -z "$journal" ]; then
  echo "skip  the mock verifier's journal is not reachable at $VERIFIER_URL"
else
  # The review requests of the first run, oldest first (the journal lists the newest first): the text of each
  # message and the context it names, which is none.
  printf '%s' "$journal" | jq --arg m "$MARKER" '
    [.requests[] | select(.request.method == "POST" and .request.url == "/a2a")
     | (.request.body | fromjson? // empty)
     | select(.method == "SendStreamingMessage" and (.params.message.parts[0].text | contains($m)))
     | {context: (.params.message.contextId // ""), text: .params.message.parts[0].text}]
    | reverse' >"$tmp/asked.json"
  expect "the verifier was asked once per verification" "$(jq 'length' "$tmp/asked.json")" "2"
  expect "naming no context: it starts a conversation of its own for each (ADR 0055)" \
    "$(jq -r '[.[].context] | join(",")' "$tmp/asked.json")" ","
  expect "never the coder's context (the thread's id)" "$(jq -r --arg t "$FLAWED" '[.[] | select(.context == $t)] | length' "$tmp/asked.json")" "0"
  expect "the first request names the commit the coder pushed" \
    "$(jq -r '.[0].text | contains("commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")' "$tmp/asked.json")" "true"
  expect "the second names the commit of the rework" \
    "$(jq -r '.[1].text | contains("commit bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")' "$tmp/asked.json")" "true"
  expect "the attempt is said" "$(jq -r '[(.[0].text | contains("attempt 1 of 3")), (.[1].text | contains("attempt 2 of 3"))] | join(",")' "$tmp/asked.json")" "true,true"
  expect "the task is quoted as data, not as an instruction" \
    "$(jq -r '.[0].text | contains("untrusted") and contains("push-flawed fix the login")' "$tmp/asked.json")" "true"
fi

echo "== a run may not choose or drop the verifier"
refused() { # refused DESCRIPTION GATE_JSON EXPECTED_DETAIL_FRAGMENT
  thread=$(uuid)
  code=$(curl -sS -o "$tmp/problem.json" -w '%{http_code}' -X POST "$BASE_URL/agui/agents/$AGENT_ID" \
    -H "$id_header" -H 'content-type: application/json' -H 'accept: text/event-stream' \
    -d "$(run_input "$thread" 'push-clean please' "$2")")
  expect "$1: 400" "$code" "400"
  expect "$1: the problem says why" \
    "$(jq -r --arg f "$3" '.detail | contains($f)' "$tmp/problem.json" 2>/dev/null || echo unreadable)" "true"
  expect "$1: no thread was created" \
    "$(curl -s -o /dev/null -w '%{http_code}' -H "$id_header" "$BASE_URL/api/threads/$thread")" "404"
}
refused "choosing another verifier" '{"verifier": "mock-coder"}' "cannot be set per thread"
refused "dropping the verifier" '{"require": []}' "may add sources"
# `ci` is honoured (slice 6), but a gate that requires it names the checks that count and this agent's has none
refused "ci with no check named" '{"require": ["verifier", "ci"]}' "no check is named"

[ "$fail" -eq 0 ] && echo "all checks passed"
exit "$fail"
