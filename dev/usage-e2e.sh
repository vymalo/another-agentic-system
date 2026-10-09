#!/usr/bin/env sh
# System-level test of token usage (usage/v1, ADR 0056, docs/api/usage-v1.md): an agent that lists the extension is asked for it,
# each model call it reports is logged once and attributed to the agent or to the sub-agent step it ran under, the task's totals are
# read from the task and logged, and the AG-UI stream says them as `vymalo.usage`, `vymalo.usage_total` and `RUN_FINISHED.usage`; an
# agent that does not list it is asked for nothing and its thread has no usage.
#
#   dev/usage-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-usage` (dev/agents.yaml), a WireMock A2A agent (dev/wiremock/usage, ours): its card lists usage/v1 and steps/v1;
# every request streams three call reports in the event metadata of `working` updates with no message (`c1`, the agent's own; `c2`
# under the sub-agent step `tool:c2`, its numbers written as doubles; `c1` again, a replay; `c3`), then `completed` with no totals on the
# update; `GetTask` answers the task with its totals in its own metadata (glm-5.3: 3600 in, 200 out; glm-5.3-mini: 600 in, 40 out). The
# plain agent is `mock-coder` (`mock-agent`), whose card does not list the extension.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the thread of `mock-usage` ends `done`; its log holds three `model_usage` (`c1`, `c2`, `c3`: the replayed `c1` once), the agent
#     `mock-usage` on each, `c2`'s path the sub-agent step (`<task>/tool:c2`) and the others' empty, `c2`'s whole doubles as integers, and one
#     `model_usage_total` with the task's two models, after the calls and before `completed`; each holds labels and numbers only;
#   * its AG-UI stream has three `CUSTOM` `vymalo.usage` (`by` the agent, the sub-agent `Researcher` under its subagent `sub-step-<seq>`,
#     the agent), one `vymalo.usage_total`, and `RUN_FINISHED.usage` equal to the task's totals;
#   * `mock-usage` was asked for the extension (the `A2A-Extensions` header and `message.extensions`) and its task was read (`GetTask`);
#   * the thread of `mock-coder` ends `done` with no `model_usage` or `model_usage_total`, no `vymalo.usage` and no `RUN_FINISHED.usage`,
#     and `mock-agent` was asked for no usage (neither the header nor the message names it).
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_USAGE_URL  http://127.0.0.1:${MOCK_USAGE_PORT:-8088}   the agent that reports its usage (WireMock admin API)
#   MOCK_CODER_URL  http://127.0.0.1:${MOCK_AGENT_PORT:-8081}   the plain agent (WireMock admin API)
#   TIMEOUT         120    seconds to wait for a thread to stop
#
# It EMPTIES the request journals of `mock-usage` and `mock-agent` first, so run it on a stack you are not in the middle of another
# scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml; the orchestrator
# side against the same WireMock agent is also `the_usage_mock_*` of orchestrator/crates/e2e/tests/wiremock_agent.rs.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
usage=${MOCK_USAGE_URL:-http://127.0.0.1:${MOCK_USAGE_PORT:-8088}}
usage=${usage%/}
coder=${MOCK_CODER_URL:-http://127.0.0.1:${MOCK_AGENT_PORT:-8081}}
coder=${coder%/}
timeout=${TIMEOUT:-120}

usage_uri=https://agents.vymalo.com/a2a/extensions/usage/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "usage e2e passed"; else echo "usage e2e FAILED"; exit 1; fi
}
expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# run_agui AGENT THREAD TEXT: one run (a message) on a new thread, to its end. Prints the HTTP status; the body is $tmp/run.sse.
run_agui() {
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

# wait_state THREAD: waits (up to TIMEOUT seconds) until the thread stops, and prints the state it ended in (or the last one it saw).
wait_state() {
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(api GET "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' || true)
    case $_state in done | blocked | failed | cancelled) echo "$_state"; return 0 ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then echo "${_state:-unknown}"; return 0; fi
    sleep 2
  done
}

# frames THREAD FILE: the thread's AG-UI events, replayed from the start, as one JSON array in FILE.
frames() {
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' "$base/agui/threads/$1/connect?mode=run" 2>/dev/null |
    sed -n 's/^data: *//p' | jq -s '.' >"$2" 2>/dev/null || echo '[]' >"$2"
}

# activations URL TEXT: for each SendStreamingMessage whose text holds TEXT, the A2A-Extensions header and the message's own extensions,
# as `<header>|<extensions>`, one line each (WireMock's journal; a header name in any case).
activations() {
  curl -s --max-time 30 "$1/__admin/requests" | jq -r --arg t "$2" '
    .requests[] | .request as $r | ($r.body | fromjson? // {}) as $b
    | select($b.method == "SendStreamingMessage" and ([$b.params.message.parts[]?.text // ""] | join(" ") | contains($t)))
    | ([$r.headers | to_entries[] | select(.key | ascii_downcase == "a2a-extensions") | .value
        | if type == "array" then join(",") else tostring end] | join(",")) + "|"
      + (($b.params.message.extensions // []) | join(","))' 2>/dev/null || true
}

rpc_count() { # rpc_count URL METHOD: how many JSON-RPC calls of METHOD the WireMock got
  curl -s --max-time 30 "$1/__admin/requests" |
    jq -r --arg m "$2" '[.requests[] | (.request.body | fromjson? // {}) | select(.method == $m)] | length' 2>/dev/null || echo '?'
}

reset_journal() { # reset_journal URL
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$1/__admin/requests" || true)
  if [ "$_code" = 200 ]; then ok "journal reset: $1"; else bad "journal reset: $1 answered HTTP $_code"; fi
}

# --- the stack ---------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
for a in mock-usage mock-coder; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done
ok "GET /api/agents lists mock-usage and mock-coder"
reset_journal "$usage"
reset_journal "$coder"
nonce=$(uuid)

# --- an agent that reports its usage -----------------------------------------------------------------------------------
echo "== mock-usage: three calls, one under a sub-agent step, and the task's totals"
thread=$(uuid)
code=$(run_agui mock-usage "$thread" "summarize the notes ($nonce)")
expect "POST /agui/agents/mock-usage answers 200" "$code" "200"
expect "the thread of mock-usage ends done" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/usage.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/usage.export.json"
expect "the log: three calls (the replayed c1 once), then the totals, then completed" \
  "$(jq -r '[.events[] | select(.kind == "model_usage" or .kind == "model_usage_total" or (.kind == "agent_status" and .data.status == "completed"))
     | if .kind == "model_usage" then .data.call elif .kind == "model_usage_total" then "total" else "completed" end] | join(",")' "$tmp/usage.export.json")" \
  "c1,c2,c3,total,completed"
task=$(jq -r '[.events[] | select(.kind == "model_usage") | .data.task] | first // ""' "$tmp/usage.export.json")
expect "each call is the agent's, mock-usage, on one task" \
  "$(jq -r '[.events[] | select(.kind == "model_usage") | "\(.data.agent)/\(.data.task)"] | unique | join(",")' "$tmp/usage.export.json")" \
  "mock-usage/$task"
expect "the call under the sub-agent step has its path, the agent's own have none" \
  "$(jq -r '[.events[] | select(.kind == "model_usage") | "\(.data.call)=\(.data.path | join("/"))"] | join(",")' "$tmp/usage.export.json")" \
  "c1=,c2=$task/tool:c2,c3="
expect "numbers sent as whole doubles are integers: c2 600 in, 40 out, 640, window 65536" \
  "$(jq -r '[.events[] | select(.kind == "model_usage" and .data.call == "c2") | .data | "\(.inputTokens) \(.outputTokens) \(.totalTokens) \(.contextWindow)"] | first // ""' "$tmp/usage.export.json")" \
  "600 40 640 65536"
expect "the totals: the task's two models, read from the task" \
  "$(jq -r '[.events[] | select(.kind == "model_usage_total") | .data | "\(.agent)/\(.task)/" + (.totals | map("\(.model):\(.inputTokens):\(.outputTokens)") | join(" "))] | join(",")' "$tmp/usage.export.json")" \
  "mock-usage/$task/glm-5.3:3600:200 glm-5.3-mini:600:40"
expect "a usage event holds labels and numbers only (no text of the conversation)" \
  "$(jq -r '[.events[] | select(.kind == "model_usage" or .kind == "model_usage_total") | .data | paths(scalars) | map(tostring) | join(".")
     | select(test("^(job|agent|task|call|path\\.[0-9]+|provider|model|inputTokens|outputTokens|totalTokens|reasoningTokens|cachedInputTokens|cacheWriteInputTokens|contextWindow|totals\\.[0-9]+\\.(provider|model|inputTokens|outputTokens|totalTokens|reasoningTokens|cachedInputTokens|cacheWriteInputTokens))$") | not)] | join(",")' "$tmp/usage.export.json")" \
  ""

frames "$thread" "$tmp/usage.frames.json"
expect "AG-UI: one vymalo.usage per call, by the agent, the sub-agent Researcher and the agent" \
  "$(jq -r '[.[] | select(.type == "CUSTOM" and .name == "vymalo.usage") | "\(.value.call):\(.value.by.kind):\(.value.by.name)"] | join(",")' "$tmp/usage.frames.json")" \
  "c1:agent:mock-usage,c2:subagent:Researcher,c3:agent:mock-usage"
expect "AG-UI: the sub-agent's call is said under its subagent" \
  "$(jq -r '[.[] | select(.type == "CUSTOM" and .name == "vymalo.usage" and .value.call == "c2") | .subagentRunId | startswith("sub-step-")] | first // false' "$tmp/usage.frames.json")" \
  "true"
expect "AG-UI: one vymalo.usage_total" \
  "$(jq -r '[.[] | select(.type == "CUSTOM" and .name == "vymalo.usage_total")] | length' "$tmp/usage.frames.json")" "1"
expect "AG-UI: RUN_FINISHED.usage is the task's totals" \
  "$(jq -r '[.[] | select(.type == "RUN_FINISHED")] | last | (.usage // []) | map("\(.model):\(.inputTokens):\(.outputTokens):\(.totalTokens)") | join(" ")' "$tmp/usage.frames.json")" \
  "glm-5.3:3600:200:3800 glm-5.3-mini:600:40:640"
asked=$(activations "$usage" "$nonce")
expect "mock-usage was asked for usage/v1, by the header and by the message" \
  "$(printf '%s\n' "$asked" | awk -F'|' -v u="$usage_uri" 'NF && index($1, u) && index($2, u) { n++ } END { print n + 0 }')" "1"
count=$(rpc_count "$usage" GetTask)
if [ "$count" != '?' ] && [ "$count" -ge 1 ]; then
  ok "the task was read for its totals (GetTask: $count)"
else
  bad "the task was not read for its totals (GetTask: $count)"
fi

# --- an agent that does not list it ------------------------------------------------------------------------------------
echo "== mock-coder: a card without usage/v1"
plain=$(uuid)
code=$(run_agui mock-coder "$plain" "add a health endpoint ($nonce)")
expect "POST /agui/agents/mock-coder answers 200" "$code" "200"
expect "the thread of mock-coder ends done" "$(wait_state "$plain")" "done"
api GET "/api/threads/$plain/export" >"$tmp/plain.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/plain.export.json"
expect "its log has no usage" \
  "$(jq -r '[.events[] | select(.kind == "model_usage" or .kind == "model_usage_total")] | length' "$tmp/plain.export.json")" "0"
frames "$plain" "$tmp/plain.frames.json"
expect "its AG-UI stream has no usage: no vymalo.usage, no RUN_FINISHED.usage (the web draws no ring)" \
  "$(jq -r '[.[] | select((.type == "CUSTOM" and (.name | startswith("vymalo.usage"))) or (.type == "RUN_FINISHED" and has("usage")))] | length' "$tmp/plain.frames.json")" "0"
asked=$(activations "$coder" "$nonce")
expect "mock-agent got the message" "$(printf '%s\n' "$asked" | awk 'NF { n++ } END { print n + 0 }')" "1"
expect "and was asked for no usage: neither the header nor the message names it" \
  "$(printf '%s\n' "$asked" | awk -v u="$usage_uri" 'NF && index($0, u) { n++ } END { print n + 0 }')" "0"

finish
