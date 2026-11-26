#!/usr/bin/env sh
# System-level test of a message's model pick against the REAL adam agent (model-pick/v1, ADR 0086,
# docs/api/model-pick-v1.md): the `chat` agent's live card lists the choices `MODEL_ALIASES_FILE` gives it, the agent
# list carries them, a thread's default and a message's pick both reach the model the alias names, and a pick the card
# does not offer is refused before anything is written or sent.
#
#   dev/pick-real-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `chat` (dev/agents.yaml), an adam-agent over dev/agents/chat/agent, on the pinned adam image
# (compose.yaml `x-adam-image`). compose.yaml mounts dev/model-profiles.json as its `MODEL_ALIASES_FILE`, which declares
# the chat's own model `mock-persona` and two aliases it offers, `mock-persona-one` and `mock-persona-two`, each with an
# endpoint label ("One", "Two"); adam's card therefore lists model-pick/v1 with those two choices, and the agent list
# carries them (the endpoint labels stay on the card: the list is the aliases and their labels). The scripted model (dev/wiremock/model, ours) answers each alias with "I answered as <model>."
# (dev/wiremock/model/mappings/persona-picked*.json), so the journal of `mock-model` says which model each run asked
# for.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/agents lists chat with pick = the two aliases, and no default (adam names none), while an agent with
#     no profiles file (researcher) has no pick member at all;
#   * a first message that names nothing is answered by the chat's own model (mock-persona);
#   * PATCH /api/threads/{id} makes `mock-persona-one` the thread's default: the next message names nothing and is
#     answered by mock-persona-one, and the log's `user_message` holds it as the resolved pick;
#   * a message that names `mock-persona-two` on that same thread wins over the thread's default: it is answered by
#     mock-persona-two, and its `user_message` names it;
#   * a message that names an alias the card does not offer is a 422 `model_not_offered` with the offered list, no
#     thread is created, and the model was sent nothing for it.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
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
timeout=${TIMEOUT:-120}

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

# models_sent TEXT: the `model` of every model request whose body holds TEXT and whose model is one of the persona
# models (the thread title and description tasks use the same mock with their own models, and their prompt holds the
# conversation too), one line each, oldest first (WireMock's journal is newest first: reversed).
models_sent() {
  curl -s --max-time 30 "$model/__admin/requests" | jq -r --arg t "$1" '
    [.requests[] | .request.body as $b | ($b | fromjson? // {}) as $j
     | select(($b // "") | contains($t))
     | ($j.model // "-") | select(startswith("mock-persona"))] | reverse | .[]' 2>/dev/null || true
}

# last_words THREAD: the detail of the thread's last completed agent status ("" when none).
last_words() {
  api GET "/api/threads/$1/export" 2>/dev/null \
    | jq -r '[.events[] | select(.kind == "agent_status" and .data.status == "completed") | .data.detail] | last // ""' \
      2>/dev/null || true
}

# picks THREAD: the pick of each user_message of the thread, as JSON (null when a message names none).
picks() {
  api GET "/api/threads/$1/export" 2>/dev/null \
    | jq -cS '[.events[] | select(.kind == "user_message") | .data.pick // null]' 2>/dev/null || echo '[]'
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
nonce=$(uuid)

# --- what the web reads ------------------------------------------------------------------------------------------------
echo "== the agent list carries the chat's choices from its live card"
api GET /api/agents >"$tmp/agents.json" 2>/dev/null || echo '[]' >"$tmp/agents.json"
expect "chat: the two aliases of MODEL_ALIASES_FILE, and no default (adam names none)" \
  "$(jq -cS '[.[] | select(.id == "chat") | .pick][0]' "$tmp/agents.json")" \
  '{"models":[{"id":"mock-persona-one","label":"mock-persona-one"},{"id":"mock-persona-two","label":"mock-persona-two"}]}'
expect "researcher: it sets no MODEL_ALIASES_FILE, so its card lists no model-pick/v1 and the web draws no chip" \
  "$(jq -r '[.[] | select(.id == "researcher")][0] | has("pick")' "$tmp/agents.json")" "false"

# --- a first message names nothing: the chat's own model answers ------------------------------------------------------
echo "== a message that names no pick is answered by the chat's own model"
thread=$(uuid)
code=$(run_agui chat "$thread" "hello there ($nonce)")
expect "POST /agui/agents/chat answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$thread")" "done"
expect "mock-model was asked with the chat's own model, mock-persona" \
  "$(models_sent "$nonce" | paste -sd, -)" "mock-persona"

# --- the thread's default ----------------------------------------------------------------------------------------------
echo "== the thread's default reaches the model, and the log"
code=$(curl -sS -o "$tmp/patch.out" -w '%{http_code}' --max-time 60 -X PATCH "$base/api/threads/$thread" -H "$id_header" \
  -H 'content-type: application/json' -d '{"model":"mock-persona-one"}')
expect "PATCH /api/threads/{id} naming mock-persona-one answers 200" "$code" "200"
expect "the thread's default is what the PATCH named" "$(jq -r '.model // "-"' "$tmp/patch.out")" "mock-persona-one"

code=$(run_agui chat "$thread" "and now with the thread's default ($nonce)")
expect "a second message that names no pick answers 200" "$code" "200"
expect "the thread ends done again" "$(wait_state "$thread")" "done"
expect "mock-model was asked with the thread's default, mock-persona-one" \
  "$(models_sent "$nonce" | paste -sd, -)" "mock-persona,mock-persona-one"
expect "the agent's words name the model it was told" "$(last_words "$thread")" "I answered as mock-persona-one."
expect "the log: the second message's pick is the thread's default" "$(picks "$thread")" \
  '[null,{"model":"mock-persona-one"}]'

# --- the message wins over the thread ----------------------------------------------------------------------------------
echo "== a message's pick wins over the thread's default"
code=$(run_agui chat "$thread" "take the other one ($nonce)" '{"vymalo.model":"mock-persona-two"}')
expect "a third message naming mock-persona-two answers 200" "$code" "200"
expect "the thread ends done a third time" "$(wait_state "$thread")" "done"
expect "mock-model was asked with mock-persona-two, not the thread's default" \
  "$(models_sent "$nonce" | paste -sd, -)" "mock-persona,mock-persona-one,mock-persona-two"
expect "the agent's words name the model the message picked" "$(last_words "$thread")" "I answered as mock-persona-two."
expect "the log: each message has its own pick, the third naming the message's" "$(picks "$thread")" \
  '[null,{"model":"mock-persona-one"},{"model":"mock-persona-two"}]'

# --- a pick the card does not offer ------------------------------------------------------------------------------------
echo "== a pick the card does not offer is refused, with the offered list"
bad_thread=$(uuid)
code=$(run_agui chat "$bad_thread" "something else please ($nonce)" '{"vymalo.model":"mock-persona-three"}')
expect "an alias the card does not offer is a 422" "$code" "422"
expect "its code is model_not_offered" "$(jq -r '.code // ""' "$tmp/run.out")" "model_not_offered"
expect "it carries the aliases the card does offer" "$(jq -cS '.offered // []' "$tmp/run.out")" \
  '["mock-persona-one","mock-persona-two"]'
expect "no thread was created" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 "$base/api/threads/$bad_thread" -H "$id_header")" "404"
expect "the model was sent nothing for the refused pick" \
  "$(models_sent "$nonce" | grep -c 'mock-persona-three' || true)" "0"

if [ "$fail" -eq 0 ]; then echo "pick (real adam) e2e passed"; else echo "pick (real adam) e2e FAILED"; exit 1; fi
