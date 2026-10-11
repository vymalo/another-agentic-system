#!/usr/bin/env sh
# System-level test of the access tier (access-tier/v1, ADR 0061, docs/api/access-tier-v1.md): an agent whose card lists the extension has
# its tiers in the capabilities document; a message that names a tier is checked, logged on the message, sent to the agent under the
# extension's URI with the extension activated, and the tier the agent echoes is logged on its status; a message that names none activates
# nothing; a tier the agent does not take, or that is not one of the four words, is refused before anything is written.
#
#   dev/tier-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-tiered` (dev/agents.yaml), a WireMock A2A agent (dev/wiremock/tiered, ours): its card lists access-tier/v1 with the
# four tiers and the default `auto`; a request that activates the extension and names a tier is answered with that tier on its task and
# its `working` update and in its last words ("Worked under the tier: plan."); any other request is answered under its default. The
# plain agent is `mock-coder` (`mock-agent`), whose card does not list the extension.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /agui/agents/mock-tiered/capabilities has `custom[<uri>] = {tiers, default}`; mock-coder's has no such key; GET /api/me names no
#     `maxTier` for the user (the roles of dev/orchestrator.yaml set no cap);
#   * a run on mock-tiered naming `plan` is accepted, its thread ends `done`; the log has the tier on the `user_message`, one `agent_status`
#     with the tier echoed and the agent's words saying it; the AG-UI stream says it in the user message's START metadata
#     (`vymalo.tier`); mock-tiered was sent the tier in the message's metadata under the URI, with the header and `message.extensions`;
#   * a second message of the thread naming `bypass` has its own tier, not the first's; a third naming none has none, and activates nothing;
#   * `yolo` is a 400, and `plan` to mock-coder is a 422 ("this agent does not take the tier 'plan'"): no thread, and mock-coder got nothing.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_TIERED_URL  http://127.0.0.1:${MOCK_TIERED_PORT:-8089}   the agent that works under a tier (WireMock admin API)
#   MOCK_CODER_URL   http://127.0.0.1:${MOCK_AGENT_PORT:-8081}    the plain agent (WireMock admin API)
#   TIMEOUT          120    seconds to wait for a thread to stop
#
# It EMPTIES the request journals of `mock-tiered` and `mock-agent` first, so run it on a stack you are not in the middle of another
# scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml; the orchestrator
# side against the same WireMock agent is also `the_tiered_mock_*` of orchestrator/crates/e2e/tests/wiremock_agent.rs.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
tiered=${MOCK_TIERED_URL:-http://127.0.0.1:${MOCK_TIERED_PORT:-8089}}
tiered=${tiered%/}
coder=${MOCK_CODER_URL:-http://127.0.0.1:${MOCK_AGENT_PORT:-8081}}
coder=${coder%/}
timeout=${TIMEOUT:-120}

tier_uri=https://agents.vymalo.com/a2a/extensions/access-tier/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "tier e2e passed"; else echo "tier e2e FAILED"; exit 1; fi
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

# run_agui AGENT THREAD TEXT [TIER]: one run (one message) on THREAD, to its end, naming TIER when it is given (a word, or `null`).
# Prints the HTTP status; the body is $tmp/run.out (the stream, or the problem).
run_agui() {
  _props='{}'
  if [ "${4:-}" != "" ]; then _props=$(jq -n --arg t "$4" 'if $t == "null" then {"vymalo.tier": null} else {"vymalo.tier": $t} end'); fi
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" --argjson props "$_props" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.out" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
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

# sent URL TEXT: for each SendStreamingMessage whose text holds TEXT, `<tier or ->|<activated by the header: yes or no>|<by the message: yes or no>`,
# one line each, oldest first (WireMock's journal, newest first: reversed; a header name in any case).
sent() {
  curl -s --max-time 30 "$1/__admin/requests" | jq -r --arg t "$2" --arg u "$tier_uri" '
    [.requests[] | .request as $r | ($r.body | fromjson? // {}) as $b
     | select($b.method == "SendStreamingMessage" and ([$b.params.message.parts[]?.text // ""] | join(" ") | contains($t)))
     | (($b.params.message.metadata[$u].tier) // "-") + "|"
       + (if ([$r.headers | to_entries[] | select(.key | ascii_downcase == "a2a-extensions") | .value
              | if type == "array" then join(",") else tostring end] | join(",") | contains($u)) then "yes" else "no" end) + "|"
       + (if (($b.params.message.extensions // []) | index($u)) != null then "yes" else "no" end)] | reverse | .[]' 2>/dev/null || true
}

reset_journal() { # reset_journal URL
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$1/__admin/requests" || true)
  if [ "$_code" = 200 ]; then ok "journal reset: $1"; else bad "journal reset: $1 answered HTTP $_code"; fi
}

# --- the stack ---------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
for a in mock-tiered mock-coder; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done
ok "GET /api/agents lists mock-tiered and mock-coder"
reset_journal "$tiered"
reset_journal "$coder"
nonce=$(uuid)

# --- what the web reads ------------------------------------------------------------------------------------------------
echo "== the capabilities and the cap"
caps() { curl -sS --max-time 30 -H "$id_header" "$base/agui/agents/$1/capabilities"; }
expect "mock-tiered: custom[access-tier/v1] has the four tiers, lowest first, and the default" \
  "$(caps mock-tiered | jq -cS --arg u "$tier_uri" '.custom[$u]')" '{"default":"auto","tiers":["plan","edits","auto","bypass"]}'
expect "mock-coder: its card does not list the extension, so the key is not there" \
  "$(caps mock-coder | jq -r --arg u "$tier_uri" '(.custom // {}) | has($u)')" "false"
expect "GET /api/me: the user's roles set no cap, so no maxTier" \
  "$(api GET /api/me | jq -r 'has("maxTier")')" "false"

# --- a message that names a tier ---------------------------------------------------------------------------------------
echo "== mock-tiered: plan, then bypass on the next message, then none"
thread=$(uuid)
code=$(run_agui mock-tiered "$thread" "fix the test, plan ($nonce)" plan)
expect "POST /agui/agents/mock-tiered naming plan answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/plan.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/plan.export.json"
expect "the log: the user_message holds the tier" \
  "$(jq -r '[.events[] | select(.kind == "user_message") | .data.tier // "-"] | first // "-"' "$tmp/plan.export.json")" "plan"
expect "the log: one agent_status holds the tier the agent echoed" \
  "$(jq -r '[.events[] | select(.kind == "agent_status" and (.data | has("tier"))) | .data.tier] | join(",")' "$tmp/plan.export.json")" "plan"
expect "the log: the agent's last words say the tier it worked under" \
  "$(jq -r '[.events[] | select(.kind == "agent_status" and .data.status == "completed") | .data.detail] | first // ""' "$tmp/plan.export.json")" \
  "Worked under the tier: plan."
frames "$thread" "$tmp/plan.frames.json"
expect "AG-UI: the user message's START says the tier in vymalo.tier" \
  "$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "user") | .metadata["vymalo.tier"] // "-"] | first // "-"' "$tmp/plan.frames.json")" "plan"
expect "mock-tiered was sent the tier in the message's metadata, with the header and message.extensions" \
  "$(sent "$tiered" "$nonce" | paste -sd, -)" "plan|yes|yes"

# a second message of the same thread names its own tier
code=$(run_agui mock-tiered "$thread" "now merge it, bypass ($nonce)" bypass)
expect "a second message naming bypass answers 200" "$code" "200"
expect "the thread ends done again" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/two.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/two.export.json"
expect "the log: each message has its own tier, plan then bypass" \
  "$(jq -r '[.events[] | select(.kind == "user_message") | .data.tier // "-"] | join(",")' "$tmp/two.export.json")" "plan,bypass"
expect "mock-tiered was sent plan, then bypass" "$(sent "$tiered" "$nonce" | paste -sd, -)" "plan|yes|yes,bypass|yes|yes"

# a message that names none activates nothing
code=$(run_agui mock-tiered "$thread" "and a last thing, no tier ($nonce)")
expect "a third message naming no tier answers 200" "$code" "200"
expect "the thread ends done a third time" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/three.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/three.export.json"
expect "the log: the third message names no tier (a tier is not carried over)" \
  "$(jq -r '[.events[] | select(.kind == "user_message") | .data.tier // "-"] | join(",")' "$tmp/three.export.json")" "plan,bypass,-"
expect "mock-tiered was sent no tier and no activation for it" \
  "$(sent "$tiered" "$nonce" | paste -sd, -)" "plan|yes|yes,bypass|yes|yes,-|no|no"

# --- a tier that is refused --------------------------------------------------------------------------------------------
echo "== a tier that is refused writes nothing"
bad_thread=$(uuid)
code=$(run_agui mock-tiered "$bad_thread" "fix the test ($nonce)" yolo)
expect "a word that is not a tier is a 400" "$code" "400"
expect "its detail names the key and the words" "$(jq -r '.detail // ""' "$tmp/run.out")" "vymalo.tier must be one of plan, edits, auto, bypass"
plain_thread=$(uuid)
code=$(run_agui mock-coder "$plain_thread" "add a health endpoint ($nonce)" plan)
expect "a tier for an agent whose card does not list the extension is a 422" "$code" "422"
expect "its detail says the agent does not take it" "$(jq -r '.detail // ""' "$tmp/run.out")" "this agent does not take the tier 'plan'"
for t in "$bad_thread" "$plain_thread"; do
  expect "no thread was created for $t" "$(curl -s -o /dev/null -w '%{http_code}' -H "$id_header" "$base/api/threads/$t")" "404"
done
expect "mock-agent was sent nothing" "$(sent "$coder" "$nonce" | wc -l | tr -d ' ')" "0"

finish
