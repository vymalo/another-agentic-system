#!/usr/bin/env sh
# Exercises the mocks of the agents' own tools over plain HTTP, one call per behaviour, so they cannot rot
# unnoticed (check-mocks.sh does the WireMock stand-in agents). For now: the mock web-search MCP server,
# `mock-mcp-search` (dev/mock-mcp-search, dev/README.md "Mock web search (MCP)"). CI runs it after
# `docker compose --profile app up -d --wait mock-mcp-search`.
#
#   dev/check-agent-mocks.sh [SEARCH_URL]     # default: http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}
#
# It plays the MCP handshake as a client does (initialize, notifications/initialized, tools/list, tools/call),
# then the answers a client has to cope with (an empty result, a tool error, a refused token, 405 on GET), and
# it EMPTIES the server's journal of calls (DELETE /__journal) before and after, so run it on a stack you are not
# in the middle of a scenario on. MOCK_MCP_TOKEN names the bearer token (default: the one of compose.yaml).
#
# Needs: curl, jq. Exit status 0 when every check passes.
set -eu

SEARCH=${1:-http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}}
TOKEN=${MOCK_MCP_TOKEN:-dev-search-token}
ICON_PREFIX='data:image/svg+xml;base64,'
fail=0

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
  "web_search:anything at all|web_search:Who won the football World Cup in 2014?|web_search:x [mock:empty]|web_search:[mock:error]|web_search:-"
check "journal: every call has a time" \
  "$(printf '%s' "$journal" | jq -r '[.calls[] | (.at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z$"))] | unique | join(",")')" "true"
check "journal: DELETE empties it" \
  "$(curl -fsS -X DELETE "$SEARCH/__journal" -o /dev/null && curl -fsS "$SEARCH/__journal" | jq -c .calls)" "[]"

[ "$fail" -eq 0 ] && echo "all checks passed"
exit "$fail"
