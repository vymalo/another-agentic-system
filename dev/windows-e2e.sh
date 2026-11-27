#!/usr/bin/env sh
# System-level test of the window tools (ADR 0092, slice 3, docs/api/windows-v1.md): the chat's scripted model makes an agent open a window, look at
# it and close it through the tools the orchestrator serves on the thread's own endpoint, on the FAKE adapter (made-up frames, no network). The agent
# is `chat` (adam-agent, a folder, on the scripted `mock-model`); nothing here talks to the fake adapter but through the orchestrator and the edge.
#
#   dev/windows-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; `chat` runs from the same image, with `adam-agent` and a folder).
# The orchestrator must be built with the Cargo feature `window-fake` (the compose file does it: `build.args.ORCH_FEATURES` of the orchestrator, and
# the coder-e2e workflow's build-args); an orchestrator without it serves no window tools and `GET /api/config` has no `windows`, which this script
# says and stops on (a SKIP, exit 77, on a machine; a FAILURE when CI or GITHUB_ACTIONS is set, or REQUIRE_WINDOWS=1):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the web does (docs/api/agui.md): one POST /agui/agents/{agentId} per message, the thread from GET /api/threads/{id}, its
# log from GET /api/threads/{id}/export. The model of `chat` is `mock-persona` (dev/wiremock/model): a message that carries `[mock:window]` selects the
# script, which, once `window_open` is among the tools it was offered, calls `window_open` on `http://93.184.216.34/` (a public address; the fake
# adapter judges it as a real one must), then `window_look` on the window the open result names, then `window_close` on the same window (the id is read
# from the requests the agent sends back, not known to the script), then says it is done; and says that no window tool is offered when `window_open`
# is not among the tools.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * `GET /api/config` says `windows: {adapters: ["fake"]}` (the capability of the process, ADR 0092 decision 8), and the capabilities of `chat` list
#     thread-tools/v1;
#   * the run ends with RUN_FINISHED, the thread ends `done` and the answer is the script's: "I opened a window, looked at it and closed it.";
#   * the model was offered the four tools, `window_open`, `window_look`, `window_act` and `window_close`, beside the built-ins, and was asked four
#     times (open, look, close, the words);
#   * THE CHAIN, read from the requests the model was sent: the result of `window_open` names a window (`w-<n>`) and a frame handed over BY REFERENCE
#     (`artifact:`, never the bytes); the model called `window_look` on that window and got a frame back; called `window_close` on the same window and
#     got `{"closed":true}`; and the last request carries that result.
# Exit status 0 when every check passed. The tools, their errors, the refusal of an asked agent and the counting adapter are the Rust tests
# (orchestrator/crates/surface-thread-tools/tests/windows.rs); this is the same chain on the stack.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL              http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL            dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL        http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   TIMEOUT               120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-model` first, so run it on a stack you are not in the middle of another scenario on. Needs curl and jq (and
# /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
timeout=${TIMEOUT:-120}
tools_uri=https://agents.vymalo.com/a2a/extensions/thread-tools/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "windows e2e passed"; else echo "windows e2e FAILED"; exit 1; fi
}
expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

api() { # api METHOD PATH [BODY]: the body on stdout, non-zero when the status is not 2xx (the resource API)
  if [ $# -ge 3 ]; then
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header" -H 'content-type: application/json' -d "$3"
  else
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
  fi
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, one JSON per line
  sed -n 's/^data: *//p' "$1"
}

# run_agui AGENT THREAD TEXT: one run (a message) on the thread, to its end. Prints the HTTP status.
run_agui() {
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

run_outcome() { # run_outcome: how the run saved by the last run_agui ended
  sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true
}

# wait_state THREAD STATE...: waits (up to TIMEOUT seconds) until the thread is in one of the states, and prints the one it ended in
# (or the last one it saw).
wait_state() {
  _thread=$1
  shift
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(api GET "/api/threads/$_thread" 2>/dev/null | jq -r '.state // empty' || true)
    for _want in "$@"; do
      if [ "$_state" = "$_want" ]; then echo "$_state"; return 0; fi
    done
    case $_state in failed | cancelled) echo "$_state"; return 0 ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then echo "${_state:-unknown}"; return 0; fi
    sleep 2
  done
}

# requests MOCK-MODEL-NAME: the bodies of the requests the model mock got for that model, as a JSON array, the oldest first (WireMock's
# journal lists the newest first).
requests() {
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c --arg m "$1" '[.requests | reverse | .[].request.body | fromjson? | select(.model == $m)]' 2>/dev/null || echo '[]'
}

# --- the stack, and what it offers ----------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" chat "*) ;;
  *) echo "the agent 'chat' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac

echo "== GET /api/config"
if ! api GET /api/config >"$tmp/config.json" 2>"$tmp/err"; then
  bad "GET /api/config: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/config.json")"
  finish
fi
adapters=$(jq -c '.windows.adapters // empty' "$tmp/config.json")
if [ -z "$adapters" ]; then
  why="GET /api/config has no \`windows\`: this orchestrator was not built with the Cargo feature window-fake (ORCH_FEATURES=window-fake, see the header)"
  # A skipped scenario is not a passing one: in CI, or with REQUIRE_WINDOWS=1, the missing feature is a failure.
  if [ -n "${CI:-}${GITHUB_ACTIONS:-}" ] || [ "${REQUIRE_WINDOWS:-}" = 1 ]; then
    echo "FAIL $why" >&2
    exit 1
  fi
  echo "SKIP  $why"
  exit 77
fi
expect "the deployment says it serves windows, on the fake adapter" "$adapters" '["fake"]'
chat_caps=$(api GET /agui/agents/chat/capabilities 2>/dev/null || echo '{}')
expect "the capabilities of chat list thread-tools/v1 (the agent is given the endpoint the tools are on)" \
  "$(printf '%s' "$chat_caps" | jq -r --arg u "$tools_uri" '(.custom // {}) | has($u)')" "true"

# --- the journal starts empty ------------------------------------------------------------------------------
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $code"; fi

# --- the run ------------------------------------------------------------------------------------------------
thread=$(uuid)
echo "== a run on thread $thread (chat): open a window, look at it, close it"
code=$(run_agui chat "$thread" "[mock:window] Open a page, look at it and close it.")
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/chat answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
expect "the run stream ended with RUN_FINISHED (success)" "$(run_outcome)" "success"
expect "the thread ended done (a chat answers, it does not wait)" "$(wait_state "$thread" "done" blocked)" "done"
api GET "/api/threads/$thread/export" >"$tmp/export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/export.json"
answer=$(jq -r '[.events[] | select(.kind == "agent_message" and .data.final == true) | .data.text] | last // empty' "$tmp/export.json")
echo "the chat said: ${answer:-<nothing>}"
case $answer in
  "I opened a window, looked at it and closed it."*) ok "the answer is the script's: it opened, looked and closed" ;;
  *"No window tools are offered"*) bad "the chat says no window tool is offered: the grant did not reach the agent, or the endpoint does not list the tools" ;;
  *) bad "the answer is not \"I opened a window, looked at it and closed it.\" (the chain stopped early)" ;;
esac

# --- the chain, from what the model was sent -------------------------------------------------------------------
echo "== what the model was sent"
persona=$(requests mock-persona)
printf '%s' "$persona" >"$tmp/model.json"
expect "four model requests: the open, the look, the close and the words" "$(jq -r 'length' "$tmp/model.json")" "4"
offered=$(jq -r '[.[0].tools // [] | .[].function.name | select(startswith("window_"))] | sort | join(" ")' "$tmp/model.json")
expect "the model was offered the four window tools" "$offered" "window_act window_close window_look window_open"
# the content of the last message of request N (a tool result), and the call the last assistant message of request N made
result_of() { jq -r --argjson n "$1" '[.[$n].messages[] | select(.role == "tool") | .content] | last // empty' "$tmp/model.json"; }
call_of() { jq -r --argjson n "$1" '[.[$n].messages[] | select(.role == "assistant" and (.tool_calls // []) != []) | .tool_calls[0].function] | last // {}' "$tmp/model.json"; }

opened=$(result_of 1)
win=$(printf '%s' "$opened" | jq -r '.window // empty' 2>/dev/null || true)
case $win in
  w-*) ok "the result of window_open names a window ($win)" ;;
  *) bad "the result of window_open names no window (tool message: '$(printf '%s' "$opened" | head -c 200)')" ;;
esac
frame_url=$(printf '%s' "$opened" | jq -r '.frame.url // empty' 2>/dev/null || true)
case $frame_url in
  artifact:*) ok "its frame is handed over by reference ($frame_url), not as bytes" ;;
  *) bad "its frame is not an artifact reference (frame.url: '$frame_url')" ;;
esac
expect "the first call was window_open on the public address" \
  "$(call_of 1 | jq -r '[.name, (.arguments | fromjson | .url)] | join(" ")')" "window_open http://93.184.216.34/"
expect "the second call was window_look on that window" \
  "$(call_of 2 | jq -r '[.name, (.arguments | fromjson | .window)] | join(" ")')" "window_look $win"
looked=$(result_of 2)
expect "window_look gave a frame back, the one of the window (the same reference: a look changes nothing)" \
  "$(printf '%s' "$looked" | jq -r '.frame.url // empty' 2>/dev/null || true)" "$frame_url"
expect "the third call was window_close on the same window" \
  "$(call_of 3 | jq -r '[.name, (.arguments | fromjson | .window)] | join(" ")')" "window_close $win"
expect "window_close answered closed" "$(result_of 3 | jq -c '.' 2>/dev/null || true)" '{"closed":true}'
no_bytes=$(printf '%s' "$persona" | grep -c 'base64' || true)
expect "no frame was sent to the model as bytes (no base64 in what it was sent)" "$no_bytes" "0"

# --- nothing off-script ---------------------------------------------------------------------------------------------
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "mock-model matched every request" "$unmatched" "0"

finish
