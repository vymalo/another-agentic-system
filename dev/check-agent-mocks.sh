#!/usr/bin/env sh
# Exercises the mocks of the agents' own tools and models over plain HTTP, one call per behaviour, so they cannot
# rot unnoticed (check-mocks.sh does the WireMock stand-in agents):
#   * the mock web-search MCP server, `mock-mcp-search` (dev/mock-mcp-search, dev/README.md "Mock web search (MCP)");
#   * the scripted models of the agents that are only a folder, on the WireMock `mock-model` (dev/wiremock/model,
#     dev/README.md "Several agents"; and `mock-title`, the title model of the orchestrator): `mock-persona` greets from the persona lines, `mock-researcher` calls
#     `search__web_search` and then names the first link of the results, and for a question that carries
#     `[mock:cards]` goes on to `ui_catalog` and `show` (a Text, three cards and a graph) before it answers. Since adam-rs
#     cf6ddbb the agents stream their model calls, so each of those scripts also has an SSE twin (`*-stream.json`, the same
#     matchers plus `"stream": true`, one priority above): the twins are played here too and must say what the plain script says.
# CI runs it after `docker compose --profile app up -d --wait mock-mcp-search mock-model`.
#
#   dev/check-agent-mocks.sh [SEARCH_URL [MODEL_URL]]
#     SEARCH_URL default: http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}
#     MODEL_URL  default: http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#
# It plays the MCP handshake as a client does (initialize, notifications/initialized, tools/list, tools/call),
# then the answers a client has to cope with (an empty result, a tool error, a refused token, 405 on GET), and
# it EMPTIES the server's journal of calls (DELETE /__journal) before and after, and the request journal of the
# model mock (DELETE /__admin/requests), so run it on a stack you are not in the middle of a scenario on.
# MOCK_MCP_TOKEN names the bearer token (default: the one of compose.yaml).
#
# Needs: curl, jq. Exit status 0 when every check passes.
set -eu

SEARCH=${1:-http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}}
MODEL=${2:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
TOKEN=${MOCK_MCP_TOKEN:-dev-search-token}
ICON_PREFIX='data:image/svg+xml;base64,'
fail=0
TMPH=$(mktemp)
TMPB=$(mktemp)
trap 'rm -f "$TMPH" "$TMPB"' EXIT

check() { # check DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then
    echo "ok    $1"
  else
    echo "FAIL  $1: expected '$3', got '$2'" >&2
    fail=1
  fi
}

# mcp BODY [CURL_ARG...]: the raw answer to a POST of the JSON-RPC message BODY, with the token and the Accept
# header the transport spec demands; extra arguments (more headers, -D -, -w ...) come after.
mcp() {
  _body=$1
  shift
  curl -sS -X POST "$SEARCH/mcp" -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -H 'accept: application/json, text/event-stream' --data-binary "$_body" "$@"
}

# status BODY [CURL_ARG...]: only the HTTP status.
status() {
  _body=$1
  shift
  mcp "$_body" -o /dev/null -w '%{http_code}' "$@"
}

rpc() { # rpc ID METHOD [PARAMS_JSON]
  jq -cn --argjson id "$1" --arg m "$2" --argjson p "${3:-null}" \
    '{jsonrpc: "2.0", id: $id, method: $m} + (if $p == null then {} else {params: $p} end)'
}

search() { # search QUERY: the tools/call message
  rpc 7 tools/call "$(jq -cn --arg q "$1" '{name: "web_search", arguments: {query: $q}}')"
}

echo "== $SEARCH (the mock web-search MCP server)"
check "healthz" "$(curl -sS -o /dev/null -w '%{http_code}' "$SEARCH/healthz")" "200"
curl -sS -X DELETE "$SEARCH/__journal" -o /dev/null

init=$(rpc 1 initialize '{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"check-agent-mocks","version":"1"}}')
check "initialize: 200, JSON, no session header" \
  "$(mcp "$init" -D - -o /dev/null | tr -d '\r' | awk 'NR == 1 {s = $2} tolower($0) ~ /^content-type: application\/json/ {j = 1} tolower($0) ~ /^mcp-session-id:/ {h = 1} END {print s, (j ? "json" : "other"), (h ? "session" : "stateless")}')" \
  "200 json stateless"
check "initialize: the requested version, the tools capability, the server's name and icon" \
  "$(mcp "$init" | jq -r '.result | [.protocolVersion, (.capabilities | keys | join("+")), .serverInfo.name, (.serverInfo.icons[0].src | startswith("'"$ICON_PREFIX"'"))] | join(" ")')" \
  "2025-11-25 tools mock-mcp-search true"
check "initialize: another version is answered with the newest this server speaks" \
  "$(mcp "$(rpc 1 initialize '{"protocolVersion":"2026-07-28"}')" | jq -r .result.protocolVersion)" "2025-11-25"
check "notifications/initialized: 202 and no body" \
  "$(status '{"jsonrpc":"2.0","method":"notifications/initialized"}' -H 'mcp-protocol-version: 2025-11-25')" "202"

tools=$(mcp "$(rpc 2 tools/list)" -H 'mcp-protocol-version: 2025-11-25')
check "tools/list: one tool, web_search, with the required query" \
  "$(printf '%s' "$tools" | jq -r '[(.result.tools | length), .result.tools[0].name, (.result.tools[0].inputSchema.required | join(","))] | join(" ")')" \
  "1 web_search query"
check "tools/list: the tool has an icon, a data: URI of an SVG" \
  "$(printf '%s' "$tools" | jq -r '.result.tools[0].icons[0] | [(.src | startswith("'"$ICON_PREFIX"'")), .mimeType] | join(" ")')" \
  "true image/svg+xml"
check "tools/list: the icon decodes to an SVG" \
  "$(printf '%s' "$tools" | jq -r '.result.tools[0].icons[0].src | ltrimstr("'"$ICON_PREFIX"'")' | base64 -d | cut -c1-4)" "<svg"

answer=$(mcp "$(search 'anything at all')")
check "tools/call: one text content, not an error" \
  "$(printf '%s' "$answer" | jq -r '[(.result.content | length), .result.content[0].type, .result.isError] | join(" ")')" "1 text false"
check "tools/call: the default results, numbered, with their links" \
  "$(printf '%s' "$answer" | jq -r '.result.content[0].text | [startswith("1. "), contains("https://example.org/mock-search/1"), contains("https://example.org/mock-search/2")] | join(" ")')" \
  "true true true"
check "tools/call: a keyword picks its own results" \
  "$(mcp "$(search 'Who won the football World Cup in 2014?')" | jq -r '.result.content[0].text | contains("https://example.org/mock-search/world-cup-2014")')" "true"
check "tools/call: the keyword async gives three results, the sources of the [mock:cards] script" \
  "$(mcp "$(search 'async programming')" | jq -r '[.result.content[0].text | scan("https://example.org/mock-search/[a-z-]+")] | join(" ")')" \
  "https://example.org/mock-search/async-book https://example.org/mock-search/async-futures https://example.org/mock-search/async-tokio"
check "tools/call: [mock:empty] -> No results." \
  "$(mcp "$(search 'x [mock:empty]')" | jq -r '[.result.content[0].text, .result.isError] | join(" ")')" "No results. false"
check "tools/call: [mock:error] -> a tool execution error" \
  "$(mcp "$(search '[mock:error]')" | jq -r '.result.isError')" "true"
check "tools/call: no query -> a tool execution error, not a protocol error" \
  "$(mcp "$(rpc 7 tools/call '{"name":"web_search","arguments":{}}')" | jq -r '[.result.isError, (.error // "none")] | join(" ")')" "true none"
check "tools/call: an unknown tool -> -32602" \
  "$(mcp "$(rpc 7 tools/call '{"name":"nope","arguments":{}}')" | jq -r .error.code)" "-32602"
check "a method it has not got -> -32601 (so a client probing server/discover falls back)" \
  "$(mcp "$(rpc 8 server/discover)" | jq -r .error.code)" "-32601"
check "ping -> {}" "$(mcp "$(rpc 9 ping)" | jq -c .result)" "{}"

check "no token -> 401" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$SEARCH/mcp" -H 'content-type: application/json' -H 'accept: application/json, text/event-stream' -d "$(rpc 1 ping)")" "401"
check "another token -> 401" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$SEARCH/mcp" -H 'Authorization: Bearer not-the-token' -H 'content-type: application/json' -H 'accept: application/json, text/event-stream' -d "$(rpc 1 ping)")" "401"
check "GET /mcp -> 405 (no SSE stream)" \
  "$(curl -s -o /dev/null -w '%{http_code}' "$SEARCH/mcp" -H "Authorization: Bearer $TOKEN" -H 'accept: text/event-stream')" "405"
check "DELETE /mcp -> 405 (no sessions)" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$SEARCH/mcp" -H "Authorization: Bearer $TOKEN")" "405"
check "Accept without text/event-stream -> 406" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$SEARCH/mcp" -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -H 'accept: application/json' -d "$(rpc 1 ping)")" "406"
check "an unsupported MCP-Protocol-Version -> 400" \
  "$(status "$(rpc 1 ping)" -H 'mcp-protocol-version: 1999-01-01')" "400"

journal=$(curl -fsS "$SEARCH/__journal")
check "journal: the calls of the tool, in order, with their arguments" \
  "$(printf '%s' "$journal" | jq -r '[.calls[] | "\(.tool):\(.arguments.query // "-")"] | join("|")')" \
  "web_search:anything at all|web_search:Who won the football World Cup in 2014?|web_search:async programming|web_search:x [mock:empty]|web_search:[mock:error]|web_search:-"
check "journal: every call has a time" \
  "$(printf '%s' "$journal" | jq -r '[.calls[] | (.at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z$"))] | unique | join(",")')" "true"
check "journal: DELETE empties it" \
  "$(curl -fsS -X DELETE "$SEARCH/__journal" -o /dev/null && curl -fsS "$SEARCH/__journal" | jq -c .calls)" "[]"

# --- the scripted models of the agents that are only a folder (WireMock, dev/wiremock/model) ---------
echo "== $MODEL (the model mock: mock-persona, mock-researcher, mock-title)"
check "model mock: health" "$(curl -sS -o /dev/null -w '%{http_code}' "$MODEL/__admin/health")" "200"
curl -sS -X DELETE "$MODEL/__admin/requests" -o /dev/null

# completion MODEL_NAME MESSAGES_JSON: the first choice of a chat completion, as JSON.
completion() {
  jq -cn --arg m "$1" --argjson msgs "$2" '{model: $m, messages: $msgs}' |
    curl -sS -X POST "$MODEL/v1/chat/completions" -H 'content-type: application/json' --data-binary @- | jq -c '.choices[0]'
}
# The system prompt of an agent that follows the persona convention (the body opens with the two lines).
system() { jq -cn --arg n "$1" --arg s "$2" '{role: "system", content: ("Your name is \($n).\nIn one sentence: \($s).\n\nYou are \($n), a test persona.")}'; }
user() { jq -cn --arg t "$1" '{role: "user", content: $t}'; }
call() { # call ID TOOL: an assistant message that calls a tool
  jq -cn --arg id "$1" --arg tool "$2" '{role: "assistant", content: null, tool_calls: [{id: $id, type: "function", function: {name: $tool, arguments: "{}"}}]}'
}
result() { jq -cn --arg id "$1" --arg c "$2" '{role: "tool", tool_call_id: $id, content: $c}'; }

greeting=$(completion mock-persona "[$(system Chat 'I chat with you and answer your questions in plain words'), $(user hi)]")
check "mock-persona: a greeting built from the two persona lines" \
  "$(printf '%s' "$greeting" | jq -r '[.message.content, .finish_reason] | join(" | ")')" \
  "Hi! I'm Chat. I chat with you and answer your questions in plain words. | stop"
check "mock-persona: any folder that follows the convention (a fourth agent): its own name and summary" \
  "$(completion mock-persona "[$(system 'Dr Who-2' 'I help with the chores'), $(user 'what can you do?')]" | jq -r .message.content)" \
  "Hi! I'm Dr Who-2. I help with the chores."
check "mock-persona: a tool result in is answered with a fixed text" \
  "$(completion mock-persona "[$(system Chat 'x'), $(user hi), $(call c1 some_tool), $(result c1 finished)]" | jq -r '.message.content | startswith("I looked into it with the tool")')" "true"

first=$(completion mock-researcher "[$(system Researcher 'I search the web for you and answer with the sources I found'), $(user 'Who won the football world cup in 2014?')]")
check "mock-researcher: the first turn calls search__web_search (id researcher-call-1)" \
  "$(printf '%s' "$first" | jq -r '[.finish_reason, .message.tool_calls[0].id, .message.tool_calls[0].function.name] | join(" ")')" \
  "tool_calls researcher-call-1 search__web_search"
check "mock-researcher: the query is the person's words (letters, digits and spaces: a template must not put a quote in JSON)" \
  "$(printf '%s' "$first" | jq -r '.message.tool_calls[0].function.arguments | fromjson | .query')" \
  "Who won the football world cup in 2014"
answer=$(completion mock-researcher "[$(system Researcher x), $(user 'Who won the football world cup in 2014?'), $(call researcher-call-1 search__web_search), $(result researcher-call-1 "$(printf '1. A (mock) — https://example.org/mock-search/world-cup-2014\n   snippet\n2. B — https://example.org/mock-search/2')")]")
check "mock-researcher: the second turn answers with the first link of the results" \
  "$(printf '%s' "$answer" | jq -r '[.finish_reason, .message.content] | join(" | ")')" \
  "stop | I searched the web for you. The best source I found is https://example.org/mock-search/world-cup-2014."
check "mock-researcher: results without a link: no source is given" \
  "$(completion mock-researcher "[$(system Researcher x), $(user q), $(call researcher-call-1 search__web_search), $(result researcher-call-1 'No results.')]" | jq -r .message.content)" \
  "I searched the web for you, but I did not find a source to give you."
check "mock-researcher: a follow-up question (a tool result is in the history, not last) searches again" \
  "$(completion mock-researcher "[$(system Researcher x), $(user first), $(call researcher-call-1 search__web_search), $(result researcher-call-1 '1. A — https://example.org/mock-search/1'), $(user 'And Rust?')]" | jq -r '[.finish_reason, (.message.tool_calls[0].function.arguments | fromjson | .query)] | join(" | ")')" \
  "tool_calls | And Rust"

# `[mock:cards]`: search, read the catalog, show cards and a graph, answer. Each turn is told by the call ids the history holds,
# and a conversation that carries the keyword never reaches the scripts above.
cards_system=$(system Researcher 'I search the web for you and answer with the sources I found')
cards_user=$(user '[mock:cards] what is async rust?')
cards_search_results=$(printf '1. A (mock) — https://example.org/mock-search/async-book\n   x\n2. B — https://example.org/mock-search/async-futures\n   y\n3. C — https://example.org/mock-search/async-tokio\n   z')
t1=$(completion mock-researcher "[$cards_system, $cards_user]")
check "mock-researcher [mock:cards]: the first turn searches for async programming (id cards-call-1)" \
  "$(printf '%s' "$t1" | jq -r '[.finish_reason, .message.tool_calls[0].id, .message.tool_calls[0].function.name, (.message.tool_calls[0].function.arguments | fromjson | .query)] | join(" | ")')" \
  "tool_calls | cards-call-1 | search__web_search | async programming"
t2=$(completion mock-researcher "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 "$cards_search_results")]")
check "mock-researcher [mock:cards]: the results are in, it reads which components the screen has (ui_catalog, cards-call-2)" \
  "$(printf '%s' "$t2" | jq -r '[.finish_reason, .message.tool_calls[0].id, .message.tool_calls[0].function.name] | join(" | ")')" \
  "tool_calls | cards-call-2 | ui_catalog"
t3=$(completion mock-researcher "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 "$cards_search_results"), $(call cards-call-2 ui_catalog), $(result cards-call-2 '{"Cards":{},"Mermaid":{}}')]")
check "mock-researcher [mock:cards]: then it shows (cards-call-3): a Text, a Cards and a Mermaid" \
  "$(printf '%s' "$t3" | jq -r '[.finish_reason, .message.tool_calls[0].id, .message.tool_calls[0].function.name, (.message.tool_calls[0].function.arguments | fromjson | [.blocks[].component] | join("+"))] | join(" | ")')" \
  "tool_calls | cards-call-3 | show | Text+Cards+Mermaid"
check "mock-researcher [mock:cards]: the three cards carry the links the search returned, and the graph is a graph TD" \
  "$(printf '%s' "$t3" | jq -r '.message.tool_calls[0].function.arguments | fromjson | [([.blocks[] | select(.component == "Cards") | .cards[].url] | join(" ")), (.blocks[] | select(.component == "Mermaid") | .code | startswith("graph TD") | tostring)] | join(" | ")')" \
  "https://example.org/mock-search/async-book https://example.org/mock-search/async-futures https://example.org/mock-search/async-tokio | true"
t4=$(completion mock-researcher "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 "$cards_search_results"), $(call cards-call-2 ui_catalog), $(result cards-call-2 '{}'), $(call cards-call-3 show), $(result cards-call-3 'Shown to the person.')]")
check "mock-researcher [mock:cards]: once show is answered it says the three links in words" \
  "$(printf '%s' "$t4" | jq -r '[.finish_reason, .message.content] | join(" | ")')" \
  "stop | Here are the three sources I found: https://example.org/mock-search/async-book, https://example.org/mock-search/async-futures and https://example.org/mock-search/async-tokio."
check "mock-researcher [mock:cards]: a refused show (the screen has no Cards) is answered in words too" \
  "$(completion mock-researcher "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 x), $(call cards-call-2 ui_catalog), $(result cards-call-2 '{}'), $(call cards-call-3 show), $(result cards-call-3 'unknown component Cards')]" | jq -r .finish_reason)" \
  "stop"

# The twins. The agents stream their model calls (adam-rs cf6ddbb: `"stream": true`, with the usage chunk asked for), so every script above has
# an SSE twin, one priority above it. Each probe is played both ways with the same messages, and what a client assembles from the stream (the
# content deltas joined, the argument deltas of each tool call joined, the finish reason) must equal what the plain script answers, so a twin
# cannot drift from its original; the stream must also be a text/event-stream that ends with [DONE], and a text must arrive in several deltas.
# streamed MODEL_NAME MESSAGES_JSON: the request with "stream": true, assembled as a client does, as {finish, content, deltas, calls}.
streamed() {
  jq -cn --arg m "$1" --argjson msgs "$2" '{model: $m, messages: $msgs, stream: true, stream_options: {include_usage: true}}' |
    curl -sS -X POST "$MODEL/v1/chat/completions" -H 'content-type: application/json' --data-binary @- |
    sed -n 's/^data: //p' | grep -v '^\[DONE\]' |
    jq -cs '[.[] | select((.choices | length) > 0) | .choices[0]] as $cs | {
      finish: ([$cs[].finish_reason | select(. != null)] | last),
      content: ([$cs[].delta.content // empty] | join("")),
      deltas: ([$cs[].delta.content // empty | select(. != "")] | length),
      calls: ([$cs[].delta.tool_calls // [] | .[]] | group_by(.index)
        | map({id: (map(.id // empty) | first), name: (map(.function.name // empty) | first), args: (map(.function.arguments // "") | join(""))}))}'
}
# plain_shape MODEL_NAME MESSAGES_JSON: the plain answer, in the shape `streamed` assembles (without `deltas`).
plain_shape() {
  completion "$1" "$2" | jq -c '{finish: .finish_reason, content: (.message.content // ""),
    calls: [(.message.tool_calls // [])[] | {id, name: .function.name, args: .function.arguments}]}'
}
# twin DESCRIPTION MODEL_NAME MESSAGES_JSON [MIN_DELTAS]: the twin says what the plain script says (and a text arrives in at least MIN_DELTAS pieces).
twin() {
  _s=$(streamed "$2" "$3")
  check "twin, $1: the stream assembles to the plain answer" \
    "$(printf '%s' "$_s" | jq -c 'del(.deltas)')" "$(plain_shape "$2" "$3")"
  if [ -n "${4:-}" ]; then
    check "twin, $1: the text arrives in at least $4 deltas" "$(printf '%s' "$_s" | jq -r --argjson n "$4" '.deltas >= $n')" "true"
  fi
}

persona_system=$(system Chat 'I chat with you and answer your questions in plain words')
curl -sS -X POST "$MODEL/v1/chat/completions" -D "$TMPH" -o "$TMPB" -H 'content-type: application/json' \
  -d "$(jq -cn --argjson msgs "[$persona_system, $(user hi)]" '{model: "mock-persona", messages: $msgs, stream: true, stream_options: {include_usage: true}}')"
check "twin: a streaming request is a text/event-stream, with the usage chunk, ending with [DONE]" \
  "$(grep -qi '^content-type: text/event-stream' "$TMPH" && echo sse || echo other) $(grep -q '^data: {.*"usage":{' "$TMPB" && echo usage || echo no-usage) $(tail -n 2 "$TMPB" | grep -q '^data: \[DONE\]' && echo finished || echo unfinished)" \
  "sse usage finished"
twin "mock-persona greets" mock-persona "[$persona_system, $(user hi)]" 2
twin "mock-persona, a fourth agent's own name and summary" mock-persona "[$(system 'Dr Who-2' 'I help with the chores'), $(user 'what can you do?')]" 2
twin "mock-persona, a tool result in" mock-persona "[$persona_system, $(user hi), $(call c1 some_tool), $(result c1 finished)]" 2
research_system=$(system Researcher 'I search the web for you and answer with the sources I found')
twin "mock-researcher searches" mock-researcher "[$research_system, $(user 'Who won the football world cup in 2014?')]"
twin "mock-researcher answers with the first link" mock-researcher \
  "[$research_system, $(user 'Who won the football world cup in 2014?'), $(call researcher-call-1 search__web_search), $(result researcher-call-1 "$(printf '1. A (mock) — https://example.org/mock-search/world-cup-2014\n   snippet\n2. B — https://example.org/mock-search/2')")]" 2
twin "mock-researcher, no link in the results" mock-researcher \
  "[$research_system, $(user q), $(call researcher-call-1 search__web_search), $(result researcher-call-1 'No results.')]" 2
twin "mock-researcher, a follow-up question searches again" mock-researcher \
  "[$research_system, $(user first), $(call researcher-call-1 search__web_search), $(result researcher-call-1 '1. A — https://example.org/mock-search/1'), $(user 'And Rust?')]"
twin "mock-researcher [mock:cards], search" mock-researcher "[$cards_system, $cards_user]"
twin "mock-researcher [mock:cards], ui_catalog" mock-researcher \
  "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 "$cards_search_results")]"
twin "mock-researcher [mock:cards], show (the arguments arrive in several deltas)" mock-researcher \
  "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 "$cards_search_results"), $(call cards-call-2 ui_catalog), $(result cards-call-2 '{"Cards":{},"Mermaid":{}}')]"
check "twin, mock-researcher [mock:cards], show: the arguments of the call are cut into several deltas" \
  "$(jq -cn --argjson msgs "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 "$cards_search_results"), $(call cards-call-2 ui_catalog), $(result cards-call-2 '{}')]" '{model: "mock-researcher", messages: $msgs, stream: true}' |
     curl -sS -X POST "$MODEL/v1/chat/completions" -H 'content-type: application/json' --data-binary @- |
     sed -n 's/^data: //p' | grep -v '^\[DONE\]' | jq -s '[.[] | select((.choices | length) > 0) | .choices[0].delta.tool_calls // [] | .[] | select(.function.arguments != "")] | length > 2')" "true"
twin "mock-researcher [mock:cards], the words" mock-researcher \
  "[$cards_system, $cards_user, $(call cards-call-1 search__web_search), $(result cards-call-1 "$cards_search_results"), $(call cards-call-2 ui_catalog), $(result cards-call-2 '{}'), $(call cards-call-3 show), $(result cards-call-3 'Shown to the person.')]" 2
check "twin: a request that does not ask for a stream still gets the plain JSON answer" \
  "$(completion mock-persona "[$persona_system, $(user hi)]" | jq -r '.message.content | startswith("Hi! I'"'"'m Chat.")')" "true"

# `mock-title`: the orchestrator's own model call (the title of a thread, ADR 0005): a title, "no topic yet", a failing model and
# a model that answers in Chinese (once, or always),
# chosen by the markers in the conversation it is shown. The agents' mocks above never answer it, and it never answers theirs.
check "mock-title: any conversation is titled \"Mock thread title\"" \
  "$(completion mock-title "$(jq -cn '[{role: "system", content: "Reply with a 3 to 6 word title"}, {role: "user", content: "Title this conversation.\n```conversation\nuser: hello\nagent: hi there\n```"}]')" | jq -r '[.finish_reason, .message.content] | join(" | ")')" \
  "stop | Mock thread title"
check "mock-title: [mock:untitled] in the conversation says NONE (no topic yet)" \
  "$(completion mock-title "$(jq -cn '[{role: "system", content: "Reply with a 3 to 6 word title"}, {role: "user", content: "```conversation\nuser: [mock:untitled] hello\n```"}]')" | jq -r .message.content)" \
  "NONE"
check "mock-title: [mock:title-error] in the conversation is a 500" \
  "$(jq -cn '{model: "mock-title", messages: [{role: "user", content: "```conversation\nuser: [mock:title-error] hello\n```"}]}' | curl -s -o /dev/null -w '%{http_code}' -X POST "$MODEL/v1/chat/completions" -H 'content-type: application/json' --data-binary @-)" "500"
# a model that drifts into Chinese, once (a WireMock scenario: its state is reset here and after), and one that is right to
curl -sS -X POST "$MODEL/__admin/scenarios/reset" -o /dev/null
chinese_body=$(jq -cn '[{role: "system", content: "Reply with a 3 to 6 word title"}, {role: "user", content: "```conversation\nuser: [mock:title-chinese] Please fix the login page\n```\nWrite the title in English."}]')
check "mock-title: the first ask about [mock:title-chinese] is answered in Chinese" \
  "$(completion mock-title "$chinese_body" | jq -r .message.content)" "绘图导出问题"
check "mock-title: the next ask about it is the default title (the Chinese answer is given once)" \
  "$(completion mock-title "$chinese_body" | jq -r .message.content)" "Mock thread title"
check "mock-title: [mock:title-zh] in the conversation is always titled in Chinese" \
  "$(completion mock-title "$(jq -cn '[{role: "user", content: "```conversation\nuser: [mock:title-zh] 请修复登录页面\n```"}]')" | jq -r .message.content)" "登录页面修复"
curl -sS -X POST "$MODEL/__admin/scenarios/reset" -o /dev/null
check "mock-title: the base path may be /chat/completions as well as /v1/chat/completions" \
  "$(jq -cn '{model: "mock-title", messages: [{role: "user", content: "hello"}]}' | curl -sS -X POST "$MODEL/chat/completions" -H 'content-type: application/json' --data-binary @- | jq -r '.choices[0].message.content')" \
  "Mock thread title"

check "an unknown model is a 404, not an invented answer" \
  "$(jq -cn '{model: "no-such-model", messages: [{role: "user", content: "hi"}]}' | curl -s -o /dev/null -w '%{http_code}' -X POST "$MODEL/v1/chat/completions" -H 'content-type: application/json' --data-binary @-)" "404"
check "the journal holds the requests, and exactly the unknown model was unmatched" \
  "$(curl -fsS "$MODEL/__admin/requests/unmatched" | jq -r '[(.requests | length), (.requests[0].body | fromjson | .model)] | join(" ")')" "1 no-such-model"
curl -sS -X DELETE "$MODEL/__admin/requests" -o /dev/null

[ "$fail" -eq 0 ] && echo "all checks passed"
exit "$fail"
