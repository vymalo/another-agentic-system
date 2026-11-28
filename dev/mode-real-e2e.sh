#!/usr/bin/env sh
# System-level test of a message's mode against the REAL adam agent (`agent-mode/v1`, ADR 0109,
# docs/api/agent-mode-v1.md): the `chat` agent's live card lists the modes of its folder
# (dev/agents/chat/agent/modes: `direct`, the default, and `plan`), the agent list carries them, a message
# that names `plan` reaches the model with that mode's instructions, and a mode the card does not offer is
# refused before anything is written or sent.
#
#   dev/mode-real-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `chat` (dev/agents.yaml), an adam-agent over dev/agents/chat/agent, on the pinned adam image
# (compose.yaml `x-adam-image`, which serves agent-mode/v1). Its folder declares two modes: `direct` (the
# default: answer straight away) and `plan` (lay out a short plan first). The scripted model
# (dev/wiremock/model, ours) receives the agent's request with the mode's instructions in the system prompt,
# so the journal of `mock-model` says which mode each run asked for.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the chat's live card lists agent-mode/v1 once, with the modes `direct` and `plan` and the default
#     `direct`, while the researcher's card (no modes folder) lists none;
#   * GET /api/agents lists chat with mode = those modes and that default, and no `mode` member for the
#     researcher;
#   * a message that names `plan` is answered and its model request carries the plan mode's instructions,
#     and the log's `user_message` holds `mode: "plan"`;
#   * a message that names no mode carries no member and the agent works in its own default (direct), whose
#     instructions are in the model request;
#   * a mode the card does not offer is a 422 `mode_not_offered` with the offered list, no thread is
#     created, and the model was sent nothing for it.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   CHAT_URL        http://127.0.0.1:${CHAT_PORT:-8097}        the chat agent (its live card)
#   CHAT_TOKEN      dev-chat-token        CHAT_A2A_TOKEN of the chat service
#   RESEARCHER_URL  http://127.0.0.1:${RESEARCHER_PORT:-8098} the agent whose folder has no modes
#   RESEARCHER_TOKEN dev-researcher-token RESEARCHER_A2A_TOKEN of the researcher
#   MOCK_MODEL_URL  http://127.0.0.1:${MOCK_MODEL_PORT:-8094}   the scripted model (WireMock admin API)
#   TIMEOUT         120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-model` first, so run it on a stack you are not in the middle of another
# scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml (dev/e2e-all.sh).
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
chated=${CHAT_URL:-http://127.0.0.1:${CHAT_PORT:-8097}}
chated=${chated%/}
chat_token=${CHAT_TOKEN:-dev-chat-token}
researched=${RESEARCHER_URL:-http://127.0.0.1:${RESEARCHER_PORT:-8098}}
researched=${researched%/}
researcher_token=${RESEARCHER_TOKEN:-dev-researcher-token}
timeout=${TIMEOUT:-120}

mode_uri=https://agents.vymalo.com/a2a/extensions/agent-mode/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
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

card() { # card URL TOKEN: the agent's card, as its own service serves it
  curl --fail-with-body -sS --max-time 30 "$1/.well-known/agent-card.json" -H "authorization: Bearer $2"
}

# run_agui AGENT THREAD TEXT [PROPS]: one run (one message) on THREAD, to its end, with forwardedProps PROPS ({ } by default).
# Prints the HTTP status; the body is $tmp/run.out (the stream, or the problem).
run_agui() {
  _props=${4:-'{}'}
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

# bodies_of TEXT MODEL: the body of every model request whose messages hold TEXT and whose model is MODEL, one JSON
# object a line, oldest first (the title and description tasks use the same mock with models of their own, and their
# prompts hold the conversation too, so the model filters them out).
bodies_of() {
  curl -s --max-time 30 "$model/__admin/requests" | jq -c --arg t "$1" --arg m "$2" '
    [.requests[] | (.request.body | fromjson? // {})
     | select(((.messages // []) | tostring) | contains($t))
     | select((.model // "-") == $m)] | reverse | .[]' 2>/dev/null || true
}

# instructions_of REQUEST: the request body as one line, for a phrase search.
instructions_of() {
  printf '%s' "$1"
}

# modes THREAD: the mode of each user_message of the thread, as JSON (null when a message names none).
modes() {
  api GET "/api/threads/$1/export" 2>/dev/null \
    | jq -cS '[.events[] | select(.kind == "user_message") | .data.mode // null]' 2>/dev/null || echo '[]'
}

# --- the stack ---------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" chat "*) ;;
  *) echo "the agent 'chat' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac
ok "GET /api/agents lists chat"
_code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$_code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $_code"; fi

# --- the cards ---------------------------------------------------------------------------------------------------------
echo "== the chat's live card lists agent-mode/v1 with the folder's two modes and the default"
chat_card=$(card "$chated" "$chat_token" 2>"$tmp/err" || true)
if [ -z "$chat_card" ]; then
  bad "the chat's card was not read from $chated/.well-known/agent-card.json: $(cat "$tmp/err")"
else
  ok "the chat's card was read"
fi
expect "the chat's card lists agent-mode/v1 exactly once" \
  "$(printf '%s' "$chat_card" | jq -r --arg u "$mode_uri" '[.. | objects | select((.uri? // "") == $u)] | length')" \
  "1"
expect "its params.modes are the folder's modes, by name" \
  "$(printf '%s' "$chat_card" | jq -c --arg u "$mode_uri" '[.. | objects | select((.uri? // "") == $u) | (.params.modes? // []) | .[] | .name] | sort')" \
  '["direct","plan"]'
expect "its params.default is the folder's default, direct" \
  "$(printf '%s' "$chat_card" | jq -r --arg u "$mode_uri" '[.. | objects | select((.uri? // "") == $u) | (.params.default? // "-")] | first // "-"')" \
  "direct"
echo "== an agent whose folder has no modes"
researcher_card=$(card "$researched" "$researcher_token" 2>"$tmp/err" || true)
if [ -z "$researcher_card" ]; then
  bad "the researcher's card was not read from $researched/.well-known/agent-card.json: $(cat "$tmp/err")"
else
  ok "the researcher's card was read"
fi
expect "the researcher's card does not list agent-mode/v1" \
  "$(printf '%s' "$researcher_card" | jq -r --arg u "$mode_uri" '[.. | objects | select((.uri? // "") == $u)] | length')" \
  "0"

# --- what the web reads ------------------------------------------------------------------------------------------------
echo "== the agent list carries the chat's modes from its live card"
api GET /api/agents >"$tmp/agents.json" 2>/dev/null || echo '[]' >"$tmp/agents.json"
expect "chat: its whole mode listing, the two modes and the default" \
  "$(jq -cS '[.[] | select(.id == "chat") | .mode] | first | .modes |= sort_by(.name)' "$tmp/agents.json")" \
  '{"default":"direct","modes":[{"description":"Answers straight away, in plain words, with no plan first.","name":"direct","title":"Direct"},{"description":"Thinks the request through and lays out a short plan before answering.","name":"plan","title":"Plan"}]}'
expect "researcher: its card lists no mode, so the web draws no switch" \
  "$(jq -r '[.[] | select(.id == "researcher")][0] | has("mode")' "$tmp/agents.json")" "false"

# --- a message names the second mode -----------------------------------------------------------------------------------
echo "== a message that names plan is answered under the plan mode"
plan_nonce=$(uuid)
plan_thread=$(uuid)
code=$(run_agui chat "$plan_thread" "what would you do? ($plan_nonce)" '{"vymalo.mode":"plan"}')
expect "a message naming plan answers 200" "$code" "200"
expect "its thread ends done" "$(wait_state "$plan_thread")" "done"
expect "the model was called for that turn" \
  "$([ -n "$(bodies_of "$plan_nonce" mock-persona | head -1)" ] && echo yes || echo no)" "yes"
expect "its system prompt carries the plan mode's instructions" \
  "$(instructions_of "$(bodies_of "$plan_nonce" mock-persona | head -1)" | grep -c 'in the \*\*plan\*\* mode' || true)" "1"
expect "the log: the message's mode is plan" "$(modes "$plan_thread")" '["plan"]'

# --- a message that names no mode --------------------------------------------------------------------------------------
echo "== a message that names no mode carries no member and the agent works in its own default"
none_nonce=$(uuid)
none_thread=$(uuid)
code=$(run_agui chat "$none_thread" "and now plainly ($none_nonce)")
expect "a message naming no mode answers 200" "$code" "200"
expect "its thread ends done" "$(wait_state "$none_thread")" "done"
expect "the model was called for that turn" \
  "$([ -n "$(bodies_of "$none_nonce" mock-persona | head -1)" ] && echo yes || echo no)" "yes"
expect "its system prompt carries the agent's own default, direct" \
  "$(instructions_of "$(bodies_of "$none_nonce" mock-persona | head -1)" | grep -c 'in the \*\*direct\*\* mode' || true)" "1"
expect "the log: the message has no mode" "$(modes "$none_thread")" '[null]'

# --- a mode the card does not offer ------------------------------------------------------------------------------------
echo "== a mode the card does not offer is refused, with the offered list"
bad_nonce=$(uuid)
bad_thread=$(uuid)
code=$(run_agui chat "$bad_thread" "something else please ($bad_nonce)" '{"vymalo.mode":"review"}')
expect "a mode the card does not offer is a 422" "$code" "422"
expect "its code is mode_not_offered" "$(jq -r '.code // ""' "$tmp/run.out")" "mode_not_offered"
expect "it carries the modes the card does offer" "$(jq -cS '.offered | sort' "$tmp/run.out")" '["direct","plan"]'
expect "no thread was created" \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 "$base/api/threads/$bad_thread" -H "$id_header")" "404"
expect "the model was sent nothing for the refused mode" \
  "$(bodies_of "$bad_nonce" mock-persona | wc -l | tr -d ' ')" "0"

if [ "$fail" -eq 0 ]; then echo "mode (real adam) e2e passed"; else echo "mode (real adam) e2e FAILED"; exit 1; fi
