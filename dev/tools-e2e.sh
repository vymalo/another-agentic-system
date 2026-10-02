#!/usr/bin/env sh
# System-level test of the tools a person attaches to a conversation (ADR 0024, MVP slice 8): a web search is attached to a chat, the
# chat's model calls it through the thread's own endpoint, the call is one step with the server's icon, and a detach takes the tool away
# from the next message. The agent is `chat` (adam-agent, a folder, on the scripted `mock-model`), the server is `websearch` of the
# `toolServers` of dev/orchestrator.yaml, which is `mock-mcp-search`; nothing here talks to either of them but through the orchestrator
# and the edge, except to read what the search was sent.
#
#   dev/tools-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; `chat` runs from the same image, with `adam-agent`
# and a folder, so there is nothing more to pull):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the web does (docs/api/agui.md): one POST /agui/agents/{agentId} per message, the thread from
# GET /api/threads/{id}, its frames from GET /agui/threads/{id}/connect?mode=run, its log from GET /api/threads/{id}/export. The model of
# `chat` is `mock-persona` (dev/wiremock/model): a message that carries `[mock:websearch]` selects the script, which calls the tool
# `websearch__web_search` when the agent offers it to the model (the relayed tool of the thread's endpoint is `<server>__<tool>`), then
# answers from its result, and answers "No web search attached" when the tool is not offered.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/tool-servers lists `websearch` with its name, its `data:` icon (the one of dev/orchestrator.yaml) and the agents it is
#     offered for (chat, coder), and nothing that is the orchestrator's alone: no URL, header, credential, allow-list or timeout;
#   * the capabilities of `chat` list thread-tools/v1 (the agent can use an attached tool) and those of the plain agent `mock-coder` have no
#     thread-tools key (a screen can say so before the person sends); a run that attaches `websearch` to `mock-coder`, for which the
#     deployment does not offer it, is 422 and creates no thread;
#   * RUN 1, a thread created with `forwardedProps["vymalo.tools"] = ["websearch"]` and "[mock:websearch] Who won the football world cup in
#     2014?": the run ends with RUN_FINISHED, the thread ends `done` and says it has `websearch` attached, the answer cites the first link
#     of the mock search, and the model was offered `websearch__web_search` and got the result back (two requests: the call, then the answer);
#   * the export (the whole log) has one `tools_attached` of `websearch`, and EXACTLY ONE tool step: two `agent_step` events of one id, the
#     start `running` and the end `completed`, with `icon: mcp-server:websearch`, the label `Web search · web_search`, the call's input (the
#     query) and its output (the mock's links); the agent reported no step of its own for the call;
#   * the frames say the same as the replay does: the `vymalo.tools` card, and one `vymalo.step` with the icon, running, then completed;
#   * the mock search was called exactly once, with that query, with the bearer token its configuration sets and the header
#     `X-Search-Tenant` with the value its configuration sets (the orchestrator holds both; the agent never sees them);
#   * no secret value (the bearer, the header's value) is in the export, the frames, the thread, the list of servers or anything the model
#     was sent;
#   * DETACH, `PUT /api/threads/{id}/tools` with an empty list: 200 and `servers: []`, the thread has none attached, and the log has one
#     `tools_detached` of `websearch`;
#   * RUN 2, "[mock:websearch] And now?" on the same thread: the thread ends `done` again and the answer is "No web search attached"
#     (the model was not offered the tool), and the mock search got no second call.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL              http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL            dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL        http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   MOCK_MCP_SEARCH_URL   http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}
#   WEBSEARCH_TOKEN       dev-search-token       the bearer the configuration sets for `websearch` (compose.yaml); a secret
#   WEBSEARCH_TENANT      dev-tenant-5c1f0a7e    the value of the header `X-Search-Tenant` it sets; a secret
#   ORCH_CONFIG           dev/orchestrator.yaml  where the icon of `websearch` is read from
#   TIMEOUT               120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-model` and the call journal of `mock-mcp-search` first, so run it on a stack you are not in the
# middle of another scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
search=${MOCK_MCP_SEARCH_URL:-http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}}
search=${search%/}
bearer=${WEBSEARCH_TOKEN:-dev-search-token}
tenant=${WEBSEARCH_TENANT:-dev-tenant-5c1f0a7e}
timeout=${TIMEOUT:-120}

root=$(cd "$(dirname "$0")/.." && pwd)
config=${ORCH_CONFIG:-$root/dev/orchestrator.yaml}
tools_uri=https://agents.vymalo.com/a2a/extensions/thread-tools/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "tools e2e passed"; else echo "tools e2e FAILED"; exit 1; fi
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

# run_agui AGENT THREAD TEXT [FORWARDED_PROPS_JSON]: one run (a message) on the thread, to its end. Prints the HTTP status, and sets
# `outcome` to how the run stream ended (success, interrupt, error: <code>, or empty).
run_agui() {
  _props=${4:-}
  [ -n "$_props" ] || _props='{}'
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" --argjson props "$_props" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
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

# snapshot THREAD NAME: the thread, its export and its frames, saved as $tmp/NAME.thread.json, NAME.export.json and NAME.frames.json
snapshot() {
  api GET "/api/threads/$1" >"$tmp/$2.thread.json" 2>/dev/null || echo '{}' >"$tmp/$2.thread.json"
  api GET "/api/threads/$1/export" >"$tmp/$2.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/$2.export.json"
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' "$base/agui/threads/$1/connect?mode=run" 2>/dev/null |
    sed -n 's/^data: *//p' | jq -s '.' >"$tmp/$2.frames.json" 2>/dev/null || echo '[]' >"$tmp/$2.frames.json"
}

# requests MOCK-MODEL-NAME: the bodies of the requests the model mock got for that model, as a JSON array, the oldest first (WireMock's
# journal lists the newest first).
requests() {
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c --arg m "$1" '[.requests | reverse | .[].request.body | fromjson? | select(.model == $m)]' 2>/dev/null || echo '[]'
}

# no_secret WHAT FILE: neither secret of the configuration is anywhere in FILE.
no_secret() {
  _found=
  for _secret in "$bearer" "$tenant"; do
    if grep -qF -- "$_secret" "$2"; then _found="$_found $(printf '%s' "$_secret" | cut -c1-8)..."; fi
  done
  if [ -z "$_found" ]; then ok "no secret value in $1"; else bad "a secret value is in $1 (starts:$_found)"; fi
}

# Every tool step of an export, in order; the steps of the agent's own work are not tool steps.
# shellcheck disable=SC2016 # jq's own variables, not the shell's
tool_steps='[.events[] | select(.kind == "agent_step" and .data.kind == "tool")]'

# --- the stack, and the agents the scenario needs ----------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
for a in chat mock-coder; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done

# --- what the deployment offers ------------------------------------------------------------------------
echo "== GET /api/tool-servers"
if api GET /api/tool-servers >"$tmp/servers.json" 2>"$tmp/err"; then
  ok "GET /api/tool-servers answered 200"
else
  bad "GET /api/tool-servers: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/servers.json")"
  finish
fi
want_icon=$(sed -n 's/^    icon: *//p' "$config" | head -n 1)
case $want_icon in
  data:image/svg+xml\;base64,*) ;;
  *) echo "no \`icon: data:image/svg+xml;base64,...\` in $config (the toolServers entry of websearch)" >&2; exit 2 ;;
esac
expect "the list has one server, websearch, named Web search" \
  "$(jq -r '[.[] | select(.id == "websearch")] | map(.name) | join(",")' "$tmp/servers.json")" "Web search"
expect "its icon is the data: URI of dev/orchestrator.yaml (drawn as it is, never fetched)" \
  "$(jq -r '[.[] | select(.id == "websearch")][0].icon // empty' "$tmp/servers.json")" "$want_icon"
expect "it is offered for chat and coder" \
  "$(jq -r '[.[] | select(.id == "websearch")][0].agents // [] | join(",")' "$tmp/servers.json")" "chat,coder"
expect "it shows nothing that is the orchestrator's alone: no URL, header, bearer, allow-list or timeout" \
  "$(jq -r '[.[] | select(.id == "websearch")][0] | keys - ["agents", "description", "icon", "id", "name"] | join(",")' "$tmp/servers.json")" ""
no_secret "the list of servers" "$tmp/servers.json"
case $(cat "$tmp/servers.json") in
  *mock-mcp-search*) bad "the list of servers names the server's address (mock-mcp-search)" ;;
  *) ok "the list of servers does not name the server's address" ;;
esac

echo "== what an agent can do with an attached tool, before anyone sends"
chat_caps=$(api GET /agui/agents/chat/capabilities 2>/dev/null || echo '{}')
expect "the capabilities of chat list thread-tools/v1" \
  "$(printf '%s' "$chat_caps" | jq -r --arg u "$tools_uri" '(.custom // {}) | has($u)')" "true"
plain_caps=$(api GET /agui/agents/mock-coder/capabilities 2>/dev/null || echo 'none')
expect "the capabilities of the plain agent mock-coder were read (a document with its identity)" \
  "$(printf '%s' "$plain_caps" | jq -r '.identity.name // "none"')" "Mock coder"
expect "and have no thread-tools key (a card that does not list the extension is never told what is attached)" \
  "$(printf '%s' "$plain_caps" | jq -r '[(.custom // {}) | keys[] | select(contains("thread-tools"))] | length')" "0"
# The deployment does not offer websearch for mock-coder: a run that attaches it is refused before anything is created.
refused=$(uuid)
code=$(run_agui mock-coder "$refused" "hello" '{"vymalo.tools":["websearch"]}')
expect "a run that attaches websearch to an agent it is not offered for is 422" "$code" "422"
expect "and creates no thread" \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -H "$id_header" "$base/api/threads/$refused")" "404"

# --- the journals start empty ------------------------------------------------------------------------------
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $code"; fi
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$search/__journal" || true)
if [ "$code" = 200 ]; then ok "journal reset: $search"; else bad "journal reset: $search answered HTTP $code"; fi

# --- run 1: a thread created with the search attached ---------------------------------------------------------
thread=$(uuid)
echo "== run 1: thread $thread (chat), websearch attached by the run that creates it"
code=$(run_agui chat "$thread" "[mock:websearch] Who won the football world cup in 2014?" '{"vymalo.tools":["websearch"]}')
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/chat answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
outcome=$(run_outcome)
expect "the run stream ended with RUN_FINISHED (success)" "${outcome:-no terminal event}" "success"
state=$(wait_state "$thread" "done" blocked)
expect "the thread ended done (a chat answers, it does not wait)" "$state" "done"
snapshot "$thread" one
expect "the thread says it has websearch attached" "$(jq -r '(.tools // []) | join(",")' "$tmp/one.thread.json")" "websearch"
answer=$(jq -r '[.events[] | select(.kind == "agent_message" and .data.final == true) | .data.text] | last // empty' "$tmp/one.export.json")
echo "the chat said: ${answer:-<nothing>}"
case $answer in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the answer cites the first link of the mock search" ;;
  *) bad "the answer does not cite https://example.org/mock-search/world-cup-2014 (the tool was not called, or its result did not go back to the model)" ;;
esac
case $answer in
  *"No web search attached"*) bad "the chat says no web search is attached: the grant did not tell the agent, or the tool was not listed on the thread's endpoint" ;;
  *) ok "the chat did not say that no web search is attached" ;;
esac

echo "== the log of the thread (the export)"
expect "the export is a thread export" "$(jq -r '.format' "$tmp/one.export.json")" "another-agentic-system/thread-export"
expect "one tools_attached event, for websearch" \
  "$(jq -r '[.events[] | select(.kind == "tools_attached") | .data.servers | join(",")] | join(" ")' "$tmp/one.export.json")" "websearch"
expect "it comes from the person (the run that created the thread)" \
  "$(jq -r '[.events[] | select(.kind == "tools_attached") | .actor.type] | join(" ")' "$tmp/one.export.json")" "user"
expect "exactly one tool step (one id, two events: nothing from the agent's own report of the call)" \
  "$(jq -r "$tool_steps"' | [(map(.data.id) | unique | length), length] | join(" ")' "$tmp/one.export.json")" "1 2"
expect "the step runs, then completes" \
  "$(jq -r "$tool_steps"' | [.[] | "\(.data.phase):\(.data.state)"] | join(" ")' "$tmp/one.export.json")" "start:running end:completed"
expect "both events carry the icon mcp-server:websearch, which the screen resolves with GET /api/tool-servers" \
  "$(jq -r "$tool_steps"' | map(.data.icon) | unique | join(" ")' "$tmp/one.export.json")" "mcp-server:websearch"
expect "the label is the server's name and the tool's: Web search · web_search" \
  "$(jq -r "$tool_steps"' | map(.data.label) | unique | join(" ")' "$tmp/one.export.json")" "Web search · web_search"
step_query=$(jq -r "$tool_steps"' | first | .data.input.query // empty' "$tmp/one.export.json")
case $step_query in
  *"world cup"*) ok "the step's start carries the call's input, the query \"$step_query\"" ;;
  *) bad "the step's start carries no input with the query (input.query: '${step_query:-none}')" ;;
esac
step_output=$(jq -r "$tool_steps"' | last | .data.output.text // empty' "$tmp/one.export.json")
case $step_output in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the step's end carries the output, the links of the mock search" ;;
  *) bad "the step's end carries no output with a link of the mock search (output.text: '${step_output:-none}')" ;;
esac
expect "the step is not an error" "$(jq -r "$tool_steps"' | last | .data.output.error // false' "$tmp/one.export.json")" "false"
expect "the tools were attached before the call (tools_attached has the lower seq)" \
  "$(jq -r "$tool_steps"' as $steps | ([.events[] | select(.kind == "tools_attached") | .seq] | first) as $attached | ($steps[0].seq > $attached) | tostring' "$tmp/one.export.json")" "true"

echo "== what a live viewer reads (the frames)"
expect "a vymalo.tools card says websearch was attached" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.tools") | .content.attached // [] | join(",")] | join(" ")' "$tmp/one.frames.json")" "websearch"
frame_steps='[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step" and .content.kind == "tool")]'
expect "one tool step in the frames, with the icon, running and then completed" \
  "$(jq -r "$frame_steps"' | [(map(.content.id) | unique | length), (map(.content.icon) | unique | join(",")), (map(.content.state) | join(">"))] | join(" ")' "$tmp/one.frames.json")" \
  "1 mcp-server:websearch running>completed"

echo "== what the search was sent"
curl -s --max-time 30 "$search/__journal" >"$tmp/journal.json" || echo '{"calls":[]}' >"$tmp/journal.json"
expect "mock-mcp-search got exactly one call" "$(jq -r '.calls | length' "$tmp/journal.json")" "1"
expect "it was web_search with the query of the step" \
  "$(jq -r '.calls[0] | "\(.tool) \(.arguments.query)"' "$tmp/journal.json")" "web_search $step_query"
expect "it carried the bearer token the configuration sets (the mock answers 401 to any other)" "$(jq -r '.calls[0].bearer' "$tmp/journal.json")" "true"
sent_tenant=$(jq -r '.calls[0].headers["x-search-tenant"] // empty' "$tmp/journal.json")
if [ "$sent_tenant" = "$tenant" ]; then
  ok "and the header the configuration sets, X-Search-Tenant, with its value"
else
  bad "the header X-Search-Tenant the search was sent is '$(printf '%s' "$sent_tenant" | cut -c1-8)...', not the value the configuration sets"
fi

echo "== what the model was sent"
persona_requests=$(requests mock-persona)
expect "two model requests, the call and then the answer" "$(printf '%s' "$persona_requests" | jq -r 'length')" "2"
offered=$(printf '%s' "$persona_requests" | jq -r '[.[0].tools // [] | .[].function.name] | join(" ")')
case " $offered " in
  *" websearch__web_search "*) ok "the model was offered websearch__web_search, the relayed tool (tools: $offered)" ;;
  *) bad "the model was offered '${offered:-no tool}', want websearch__web_search among them (did the grant of the thread's tools reach chat? is the server attached for it?)" ;;
esac
result=$(printf '%s' "$persona_requests" | jq -r '[.[1].messages // [] | .[] | select(.role == "tool") | .content] | last // empty')
case $result in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the results of the search went back to the model" ;;
  *) bad "the second model request holds no search result (tool message: '${result:-none}')" ;;
esac
printf '%s' "$persona_requests" >"$tmp/model.json"

echo "== no secret anywhere the system keeps or says"
no_secret "the export" "$tmp/one.export.json"
no_secret "the frames" "$tmp/one.frames.json"
no_secret "the thread" "$tmp/one.thread.json"
no_secret "what the model was sent" "$tmp/model.json"

# --- detach -------------------------------------------------------------------------------------------------------
echo "== detach: PUT /api/threads/$thread/tools with an empty list"
if api PUT "/api/threads/$thread/tools" '{"servers":[]}' >"$tmp/put.json" 2>"$tmp/err"; then
  ok "PUT /api/threads/{id}/tools answered 200"
else
  bad "PUT /api/threads/{id}/tools: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/put.json")"
fi
expect "the set after the change is empty" "$(jq -c '.servers' "$tmp/put.json" 2>/dev/null || echo none)" "[]"
expect "the thread has none attached" "$(api GET "/api/threads/$thread" | jq -r '(.tools // []) | length')" "0"
api GET "/api/threads/$thread/export" >"$tmp/detached.export.json"
expect "the log has one tools_detached event, for websearch" \
  "$(jq -r '[.events[] | select(.kind == "tools_detached") | .data.servers | join(",")] | join(" ")' "$tmp/detached.export.json")" "websearch"
last_user=$(jq -r '[.events[] | select(.kind == "user_message") | .seq] | last' "$tmp/detached.export.json")

# --- run 2: the same thread, nothing attached ---------------------------------------------------------------------
echo "== run 2: the same thread, with no search attached"
code=$(run_agui chat "$thread" "[mock:websearch] And now?")
expect "the second message is accepted" "$code" "200"
state=$(wait_state "$thread" "done" blocked)
expect "the thread ended done again" "$state" "done"
snapshot "$thread" two
answer=$(jq -r --argjson s "$last_user" '[.events[] | select(.seq > $s and .kind == "agent_message" and .data.final == true) | .data.text] | last // empty' "$tmp/two.export.json")
echo "the chat said: ${answer:-<nothing>}"
case $answer in
  "No web search attached"*) ok "the answer is: No web search attached" ;;
  *) bad "the answer to the second message is not \"No web search attached...\" (the model was still offered the tool, or the run did not answer)" ;;
esac
persona_requests=$(requests mock-persona)
expect "the model got one more request, and no tool result was the last message of it" \
  "$(printf '%s' "$persona_requests" | jq -r '[length, (.[2].messages[-1].role // "none")] | join(" ")')" "3 user"
offered=$(printf '%s' "$persona_requests" | jq -r '[.[2].tools // [] | .[].function.name] | join(" ")')
case " $offered " in
  *" websearch__web_search "*) bad "the model was offered websearch__web_search after the detach (tools: $offered)" ;;
  *) ok "the model was not offered websearch__web_search after the detach (tools: ${offered:-none})" ;;
esac
expect "the mock search got no second call" "$(curl -s --max-time 30 "$search/__journal" | jq -r '.calls | length')" "1"
expect "still exactly one tool step in the whole log" \
  "$(jq -r "$tool_steps"' | map(.data.id) | unique | length' "$tmp/two.export.json")" "1"
no_secret "the export, at the end" "$tmp/two.export.json"
no_secret "the frames, at the end" "$tmp/two.frames.json"

# --- nothing off-script ---------------------------------------------------------------------------------------------
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "mock-model matched every request" "$unmatched" "0"

finish
