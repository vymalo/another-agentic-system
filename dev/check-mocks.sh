#!/usr/bin/env sh
# Exercises the WireMock stand-in agents of compose.yaml over plain HTTP, one call per scenario,
# so the mocks cannot rot unnoticed. CI runs it after `docker compose up -d --wait`.
#
#   dev/check-mocks.sh [AGENT_URL [RELEASES_URL]]     # defaults: http://127.0.0.1:8081, :8082
#
# It also plays the verification scenarios of the first mock (`red-once`, `red-always`; dev/README.md
# "Verification"): the artifacts `branch` and `checks` an agent reports for the gate, and how the
# rework prompt of the gate (which says "this is attempt N" and quotes the findings) changes the answer.
#
# Needs: curl, jq. Exit status 0 when every check passes.
set -eu

AGENT=${1:-http://127.0.0.1:8081}
RELEASES=${2:-http://127.0.0.1:8082}
EXT=https://agents.vymalo.com/a2a/extensions/release-channels/v1
fail=0

check() { # check DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then
    echo "ok    $1"
  else
    echo "FAIL  $1: expected '$3', got '$2'" >&2
    fail=1
  fi
}

# rpc BASE METHOD [TEXT] [TASK_ID] [RELEASE]: the raw answer to a JSON-RPC call with a bearer token.
rpc() {
  jq -n --arg m "$2" --arg t "${3:-hello}" --arg task "${4:-}" --arg rel "${5:-}" --arg ext "$EXT" '
    {jsonrpc: "2.0", id: "check-1", method: $m,
     params: (if ($m == "GetTask" or $m == "CancelTask" or $m == "SubscribeToTask") then {id: "task-check"}
              else {message: ({messageId: "check-msg", contextId: "check-ctx", role: "ROLE_USER", parts: [{text: $t}]}
                      + (if $task == "" then {} else {taskId: $task} end)
                      + (if $rel == "" then {} else {metadata: {($ext): {release: $rel}}} end))} end)}' |
    curl -fsS -X POST "$1/a2a" -H 'Authorization: Bearer dev-mock-token' \
      -H 'content-type: application/json' --data-binary @-
}

# frames BASE TEXT [TASK_ID] [RELEASE]: the states of a SendStreamingMessage answer, in order.
frames() {
  rpc "$1" SendStreamingMessage "$2" "${3:-}" "${4:-}" |
    sed -n 's/^data: //p' |
    jq -r '.result | (.task // .statusUpdate // .artifactUpdate) as $e |
           if .artifactUpdate then "artifact" else ($e.status.state | sub("TASK_STATE_"; "") | ascii_downcase) end' |
    paste -sd, -
}

for base in "$AGENT" "$RELEASES"; do
  echo "== $base"
  card=$(curl -fsS "$base/.well-known/agent-card.json")
  check "card: streaming, JSONRPC interface on the same host" \
    "$(printf '%s' "$card" | jq -r '[.capabilities.streaming, (.supportedInterfaces[0].url | startswith("'"$base"'/")), .supportedInterfaces[0].protocolVersion] | join(",")')" \
    "true,true,1.0"
  check "card: bearer security scheme" \
    "$(printf '%s' "$card" | jq -r '.securitySchemes.bearer.httpAuthSecurityScheme.scheme')" "Bearer"
  check "no token -> 401" \
    "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$base/a2a" -d '{}')" "401"
  check "default script" "$(frames "$base" 'add a health endpoint')" "submitted,working,artifact,completed"
  check "keyword ask -> input-required" "$(frames "$base" 'please ask me')" "submitted,working,input_required"
  check "follow-up on the task completes it" "$(frames "$base" 'main' task-check-msg)" "working,artifact,completed"
  check "keyword fail -> failed" "$(frames "$base" 'fail please')" "submitted,working,failed"
  check "keyword reject -> rejected" "$(frames "$base" 'reject it')" "submitted,rejected"
  check "keyword error -> JSON-RPC error -32602" \
    "$(rpc "$base" SendStreamingMessage 'error out' | jq -r .error.code)" "-32602"
  check "JSON-RPC id is echoed" "$(rpc "$base" SendMessage hello | jq -r .id)" "check-1"
  check "SendMessage -> completed task with the PR artifact" \
    "$(rpc "$base" SendMessage hello | jq -r '[.result.task.status.state, .result.task.artifacts[0].parts[0].url] | join(" ")')" \
    "TASK_STATE_COMPLETED https://github.com/example/sandbox/pull/1"
  check "SendMessage keyword ask -> input-required" \
    "$(rpc "$base" SendMessage 'ask' | jq -r .result.task.status.state)" "TASK_STATE_INPUT_REQUIRED"
  check "GetTask -> completed, id echoed" \
    "$(rpc "$base" GetTask | jq -r '[.result.id, .result.status.state] | join(" ")')" "task-check TASK_STATE_COMPLETED"
  check "CancelTask -> canceled" "$(rpc "$base" CancelTask | jq -r .result.status.state)" "TASK_STATE_CANCELED"
  check "SubscribeToTask -> task not found (-32001)" "$(rpc "$base" SubscribeToTask | jq -r .error.code)" "-32001"
  check "unknown method -> -32601" "$(rpc "$base" 'message/send' | jq -r .error.code)" "-32601"
done

echo "== $AGENT (verification scenarios)"
# artifact BASE TEXT NAME JQ: a jq expression over the data part of the artifact NAME of the answer.
artifact() {
  rpc "$1" SendStreamingMessage "$2" |
    sed -n 's/^data: //p' |
    jq -r --arg n "$3" '.result.artifactUpdate.artifact | select(. != null and .name == $n) | .parts[0].data | '"$4"
}
# The prompt the gate sends an agent whose work failed (orch-core, verify.rs), with one finding in it.
rework() { # rework ATTEMPT SCENARIO
  printf 'Your work did not pass verification (attempt %s of 3); this is attempt %s. Fix what is reported below.\n\n### the checks of the agent\n- %s: tests::login fails: expected 200, got 500\n' "$(($1 - 1))" "$1" "$2"
}
check "red-once: attempt 1 streams a branch and checks, then completes" \
  "$(frames "$AGENT" 'red-once fix the login')" "submitted,working,artifact,artifact,completed"
check "red-once: attempt 1 pushes a commit" \
  "$(artifact "$AGENT" 'red-once fix the login' branch '[.branch, (.commit | .[0:7])] | join(" ")')" "agent/red-once 1111111"
check "red-once: attempt 1 checks fail, with a finding" \
  "$(artifact "$AGENT" 'red-once fix the login' checks '[.passed, .commit[0:7], .findings[0]] | join(" ")')" \
  "false 1111111 red-once: tests::login fails: expected 200, got 500"
check "red-once: the rework prompt of attempt 2 gets passing checks on another commit" \
  "$(artifact "$AGENT" "$(rework 2 red-once)" checks '[.passed, .commit[0:7], (.findings // [] | length)] | join(" ")')" "true 2222222 0"
check "red-once: attempt 3 passes too" \
  "$(artifact "$AGENT" "$(rework 3 red-once)" checks '.passed')" "true"
check "red-always: attempt 1 fails" \
  "$(artifact "$AGENT" 'red-always fix the login' checks '[.passed, .findings[0]] | join(" ")')" \
  "false red-always: tests::login fails: expected 200, got 500"
check "red-always: every rework fails again" \
  "$(artifact "$AGENT" "$(rework 3 red-always)" checks '[.passed, .commit[0:7]] | join(" ")')" "false 3333333"
check "red-always: the stream is branch, checks, completed like the others" \
  "$(frames "$AGENT" "$(rework 2 red-always)")" "submitted,working,artifact,artifact,completed"

echo "== $RELEASES (release-channels extension)"
check "card declares the extension with channels and revisions" \
  "$(curl -fsS "$RELEASES/.well-known/agent-card.json" |
    jq -r --arg ext "$EXT" '.capabilities.extensions[] | select(.uri == $ext) | [.params.defaultChannel, .params.channels.staging, (.params.revisions | map(.name) | join("/"))] | join(",")')" \
  "production,coder-r51,coder-r53/coder-r51/coder-r47"
echo_revision() { # echo_revision RELEASE -> "requested revision"
  rpc "$RELEASES" SendStreamingMessage 'ship it' '' "$1" | sed -n 's/^data: //p' | head -1 |
    jq -r --arg ext "$EXT" '.result.task.metadata[$ext] | "\(.requested) \(.revision)"'
}
check "channel staging resolves to its revision" "$(echo_revision staging)" "staging coder-r51"
check "an exact revision is accepted" "$(echo_revision coder-r53)" "coder-r53 coder-r53"
check "unknown release fails closed" "$(frames "$RELEASES" 'ship it' '' nope)" "submitted,failed"

[ "$fail" -eq 0 ] && echo "all checks passed"
exit "$fail"
