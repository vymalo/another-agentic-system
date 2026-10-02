#!/usr/bin/env sh
# System-level test of the researcher answering with cards and a graph (MVP slice 4, ADR 0023): the screen's UI
# catalog (version 3, with `Cards` and `Mermaid`) goes to the researcher with the first message, the researcher
# searches, reads which components the screen has, and answers with ONE A2UI surface beside its words: a Text, a
# Cards of the three sources it found and a Mermaid graph, under the screen's own catalogId. A thread keeps its
# catalog when an older screen writes to it, and a screen whose catalog has no `Cards` gets words only.
#
#   dev/cards-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; the researcher is `adam-agent`
# from the same image, on a folder, a scripted model and the mock web search):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the web does (docs/api/agui.md): one POST /agui/agents/researcher per run, the thread's
# state from GET /api/threads/{id}, its frames from GET /agui/threads/{id}/connect?mode=run, its log from
# GET /api/threads/{id}/export. The catalog it sends is the one the web ships, read from
# web/src/features/chat/lib/a2ui/catalog/catalog.json and catalog.lock.json (`forwardedProps["vymalo.uiCatalog"]`).
# The researcher's model is `mock-researcher` (dev/wiremock/model/mappings/researcher-cards.json): a question that
# carries `[mock:cards]` makes it call `search__web_search` ("async programming", whose results the mock web search
# keeps under the keyword `async`: dev/mock-mcp-search/results.json), `ui_catalog`, `show` (a Text, a Cards of three
# cards with those links, a Mermaid `graph TD`) and then answer in words that name the three links.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the screen's catalog is what its lock says (the digest computed here with jq and sha256sum), the researcher is
#     listed by GET /api/agents and its card lists A2UI v0.9.1 (with inline catalogs), ui-catalog/v1 and thread-tools/v1;
#   * RUN 1, "[mock:cards] what is async rust?" with the catalog (version 3): RUN_FINISHED, the thread `done`; one agent
#     message that names the three links; exactly one `a2ui-surface`, under the screen's catalogId, whose components
#     are a Column, a Text, a Cards of three cards whose links are the ones the search returned, and a Mermaid
#     `graph TD`; `thread.uiCatalog` says version 3; the mock web search got exactly one call (`web_search`, a query
#     with "async"); the model was offered `search__web_search`, `ui_catalog` and `show` (not `get_ui_catalog`: one catalog
#     tool) and its log says it could list the thread tools, got the
#     components of the screen from `ui_catalog` (Cards and Mermaid among them) and "Shown to the person." from `show`;
#   * RUN 2, the same thread from an OLDER screen: such a screen sends no catalog (it sends one only when the thread has
#     none or its own is newer, docs/api/ui-catalog-v1.md), so the run adds no `ui_catalog` to the thread's log, the
#     thread's catalog stays version 3, and the run does not fail and ends in words that name the links;
#   * RUN 3, a new thread from a screen that is still on a catalog without `Cards` and `Mermaid` (the shipped one minus
#     those two, one version down, digest recomputed): the thread's catalog is that one, the model's `show` is refused,
#     and the researcher answers in words only (the three links, no `a2ui-surface`);
#   * the researcher's model mock answered every request (no error, none unmatched).
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL            http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL          dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_MODEL_URL      http://127.0.0.1:${MOCK_MODEL_PORT:-8094}, the researcher's model
#   MOCK_MCP_SEARCH_URL http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}, the researcher's search
#   RESEARCHER_URL      http://127.0.0.1:${RESEARCHER_PORT:-8098}, the researcher itself, only to read its card
#   CATALOG_FILE        web/src/features/chat/lib/a2ui/catalog/catalog.json        the screen's catalog
#   CATALOG_LOCK        web/src/features/chat/lib/a2ui/catalog/catalog.lock.json   its {version, digest}
#   SEARCH_RESULTS      dev/mock-mcp-search/results.json    the canned results; the links of `keywords.async` are expected
#   TIMEOUT             120    seconds to wait for a run to end
#
# It EMPTIES the request journal of `mock-model` and the call journal of `mock-mcp-search` first, so run it on a stack
# you are not in the middle of another scenario on. Needs curl, jq and sha256sum (or shasum), and /proc or uuidgen for a
# UUID. Verified by CI only, in .github/workflows/coder-e2e.yml.
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
researcher=${RESEARCHER_URL:-http://127.0.0.1:${RESEARCHER_PORT:-8098}}
researcher=${researcher%/}
timeout=${TIMEOUT:-120}

root=$(cd "$(dirname "$0")/.." && pwd)
# when this run began: the agent's log is read from here on (grant_reached)
started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
catalog_file=${CATALOG_FILE:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.json}
catalog_lock=${CATALOG_LOCK:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.lock.json}
results_file=${SEARCH_RESULTS:-$root/dev/mock-mcp-search/results.json}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "cards e2e passed"; else echo "cards e2e FAILED"; exit 1; fi
}

for f in "$catalog_file" "$catalog_lock" "$results_file"; do
  if [ ! -f "$f" ]; then
    echo "FAIL $f does not exist (CATALOG_FILE, CATALOG_LOCK and SEARCH_RESULTS name the screen's catalog, its lock and the mock search's results)"
    exit 1
  fi
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, one JSON per line
  sed -n 's/^data: *//p' "$1"
}

sha256_hex() { # the SHA-256 of stdin, as 64 lower-case hex digits
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d ' ' -f 1; else shasum -a 256 | cut -d ' ' -f 1; fi
}

# canonical_digest FILE: "sha256:" and the hash of the canonical JSON of FILE: keys sorted, no whitespace, no trailing
# newline. For a catalog (ASCII keys, integers) this is what the web computes (docs/api/ui-catalog-v1.md, "Digest").
canonical_digest() {
  printf 'sha256:%s' "$(printf '%s' "$(jq -cS . "$1")" | sha256_hex)"
}

# --- the screen's catalog, and an older screen's ---------------------------------------------------------------
catalog_id=$(jq -r '.catalogId' "$catalog_file")
version=$(jq -r '.version' "$catalog_lock")
digest=$(jq -r '.digest' "$catalog_lock")
echo "the screen's catalog: $catalog_id, version $version, $digest"
computed=$(canonical_digest "$catalog_file")
if [ "$computed" = "$digest" ]; then
  ok "the lock matches the catalog (the digest computed here with jq and sha256sum is the lock's)"
else
  bad "the lock says $digest, the catalog's canonical form hashes to $computed here (is catalog.lock.json out of date, or is this jq's canonical form not the web's?)"
fi
if ! jq -e '.components | has("Cards") and has("Mermaid")' "$catalog_file" >/dev/null 2>&1; then
  bad "the shipped catalog has no Cards or no Mermaid: this scenario needs catalog version 3 or later"
  finish
fi
# The catalog of a screen that was built before Cards and Mermaid: the shipped one without them, one version down.
old_version=$((version - 1))
jq 'del(.components.Cards, .components.Mermaid)' "$catalog_file" > "$tmp/old.json"
old_digest=$(canonical_digest "$tmp/old.json")
echo "an older screen's catalog: version $old_version, $old_digest (without Cards and Mermaid)"
# The links the mock search returns for the model's query: what the cards must carry.
expected=$(jq -r '.keywords.async | map(.url) | join(" ")' "$results_file")
if [ -z "$expected" ] || [ "$expected" = null ]; then
  echo "FAIL $results_file has no keyword 'async' (dev/mock-mcp-search/results.json)"
  exit 1
fi

# --- the agent and its card -------------------------------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  if printf '%s' "$agents" | jq -e 'any(.[]; .id == "researcher")' >/dev/null 2>&1; then
    ok "GET /api/agents lists researcher"
  else
    bad "GET /api/agents does not list researcher (agents: $(printf '%s' "$agents" | jq -c '[.[].id]'))"
    finish
  fi
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi
a2ui_uri=https://a2ui.org/a2a-extension/a2ui/v0.9.1
ui_uri=https://agents.vymalo.com/a2a/extensions/ui-catalog/v1
tools_uri=https://agents.vymalo.com/a2a/extensions/thread-tools/v1
code=$(curl -s -o "$tmp/card.json" -w '%{http_code}' --max-time 30 "$researcher/.well-known/agent-card.json" || true)
if [ "$code" != 200 ]; then
  bad "the researcher's card: GET $researcher/.well-known/agent-card.json answered HTTP ${code:-none} (is the researcher's port published? RESEARCHER_URL names it)"
else
  for uri in "$a2ui_uri" "$ui_uri" "$tools_uri"; do
    if jq -e --arg u "$uri" '.capabilities.extensions[]? | select(.uri == $u)' "$tmp/card.json" >/dev/null 2>&1; then
      ok "the researcher's card lists $uri"
    else
      bad "the researcher's card does not list $uri (is the image pinned in compose.yaml an adam-rs commit with cards and Mermaid, c13ddf1 or later?)"
    fi
  done
  if jq -e --arg u "$a2ui_uri" '.capabilities.extensions[]? | select(.uri == $u) | .params.acceptsInlineCatalogs == true' "$tmp/card.json" >/dev/null 2>&1; then
    ok "the A2UI entry of the card accepts inline catalogs"
  else
    bad "the A2UI entry of the card does not say acceptsInlineCatalogs: true"
  fi
fi

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $code"; fi
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$search/__journal" || true)
if [ "$code" = 200 ]; then ok "journal reset: $search"; else bad "journal reset: $search answered HTTP $code"; fi

# --- one run ------------------------------------------------------------------------------------------------
thread=
n=0

# stream INPUT_FILE LABEL: POST the RunAgentInput to the researcher and wait for the run to end. Sets
#   outcome   how the run stream ended (success, interrupt, error: <code>, or empty)
#   state     the state the thread ended in
#   said      the words the researcher spoke in this run (the assistant messages of its stream), joined
#   messages  how many assistant messages this run's stream held
#   events    the file holding every frame of the thread so far (a JSON array)
stream() {
  _deadline=$(( $(date +%s) + timeout ))
  _code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/researcher" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary "@$1" 2>"$tmp/err" || true)
  if [ "$_code" != 200 ]; then
    bad "$2: POST /agui/agents/researcher answered HTTP ${_code:-none}: $(head -c 300 "$tmp/err") $(head -c 400 "$tmp/run.sse" 2>/dev/null)"
    finish
  fi
  outcome=$(sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true)
  # The words of each assistant message as a client keeps them, the messages joined with a space. Since adam-rs cf6ddbb the
  # answer is shown while it is written (text-stream/v1, docs/api/agui.md, "Live text"): the run stream holds it as several
  # deltas, a live one continuing from its `metadata["vymalo.live"].offset` (UTF-16 code units; the mock's words are ASCII, so
  # they are jq's string positions) and the log's final one completing the same message, so deltas are not joined by a space.
  said=$(sse_events "$tmp/run.sse" | jq -rs 'reduce .[] as $f ({order: [], text: {}};
      if $f.type == "TEXT_MESSAGE_START" and $f.role == "assistant" then
        (if (.text | has($f.messageId)) then . else (.order += [$f.messageId] | .text[$f.messageId] = "") end)
      elif $f.type == "TEXT_MESSAGE_CONTENT" and (.text | has($f.messageId)) then
        .text[$f.messageId] |= ((if $f.metadata["vymalo.live"].offset != null then .[0:$f.metadata["vymalo.live"].offset] else . end) + $f.delta)
      else . end)
    | [.order[] as $id | .text[$id]] | join(" ")' 2>/dev/null || true)
  messages=$(sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "assistant")] | length' 2>/dev/null || echo '?')
  state=
  while :; do
    state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
    case $state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 2
  done
  events=$tmp/events.json
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
    "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
    echo '[]' > "$events"
  echo "$2: the researcher said: ${said:-<nothing>}"
}

# why EVENTS: what the thread said about a failure, to help whoever reads the log.
why() {
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$1" | head -n 20
}

# input_message TEXT [VERSION DIGEST CATALOG_FILE]: the RunAgentInput of a run with one new user message in $thread and,
# with the three more arguments, the catalog the screen sends under forwardedProps["vymalo.uiCatalog"] (the file is read as
# jq's input, so `.` is the catalog). An older screen sends none.
input_message() {
  n=$((n + 1))
  if [ "$#" -lt 4 ]; then
    jq -nc --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [],
      messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}'
    return
  fi
  jq -c --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" \
    --argjson version "$2" --arg digest "$3" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [],
      messages: [{id: $msg, role: "user", content: $text}],
      forwardedProps: {"vymalo.uiCatalog": {catalogId: .catalogId, version: $version, digest: $digest, catalog: .}}}' "$4"
}

# requests: the bodies and statuses of the requests the researcher's model mock got for mock-researcher, oldest first
# (WireMock's journal lists the newest first), as a JSON array of {status, body}.
requests() {
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c '[.requests | reverse | .[] | {status: .response.status, body: (.request.body | fromjson? // {})} | select(.body.model == "mock-researcher")]' 2>/dev/null || echo '[]'
}

# tool_result REQUESTS CALL_ID: what the last request of REQUESTS that holds a tool message for CALL_ID says it was.
tool_result() {
  printf '%s' "$1" | jq -r --arg id "$2" '[.[] | .body.messages // [] | .[] | select(.role == "tool" and .tool_call_id == $id) | .content] | last // empty'
}

# held: the catalog the thread's state says it holds, in the last STATE_SNAPSHOT of $events: "<catalogId> <version> <digest>".
held() {
  jq -r '[.[] | select(.type == "STATE_SNAPSHOT") | .snapshot.thread.uiCatalog // empty] | last // {}
         | [.catalogId, (.version | tostring), .digest] | join(" ")' "$events" 2>/dev/null || echo ''
}

# catalogs_in_log: the ui_catalog events of the thread's log, as a JSON array of their data.
catalogs_in_log() {
  if api GET "/api/threads/$thread/export" > "$tmp/export.json" 2>"$tmp/err"; then
    jq -c '[.events[] | select(.kind == "ui_catalog") | .data]' "$tmp/export.json"
  else
    echo "null"
  fi
}

# --- RUN 1: cards and a graph on a screen that draws them --------------------------------------------------------
thread=$(uuid)
echo "thread $thread"
echo
echo "run 1: [mock:cards] with the screen's catalog (version $version)"
input_message '[mock:cards] what is async rust?' "$version" "$digest" "$catalog_file" > "$tmp/run1.json"
stream "$tmp/run1.json" "run 1"
if [ "$outcome" = success ]; then
  ok "run 1: the run stream ended with RUN_FINISHED (success)"
else
  bad "run 1: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"
fi
if [ "$state" = "done" ]; then
  ok "run 1: the thread ended done"
else
  bad "run 1: the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"
  why "$events"
fi
if [ "$messages" = 1 ]; then
  ok "run 1: one agent message"
else
  bad "run 1: $messages agent messages, want one"
fi
for url in $expected; do
  case $said in
    *"$url"*) ok "run 1: the words name $url" ;;
    *) bad "run 1: the words do not name $url" ;;
  esac
done
surfaces=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface") | .messageId] | unique | length' "$events" 2>/dev/null || echo '?')
if [ "$surfaces" = 1 ]; then
  ok "run 1: exactly one a2ui-surface"
else
  bad "run 1: $surfaces a2ui-surface activities, want exactly one"
fi
# The last snapshot of the surface holds all of it (replace: true).
operations=$(jq -c '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface")] | last | .content.a2ui_operations // []' "$events" 2>/dev/null || echo '[]')
surface_catalog=$(printf '%s' "$operations" | jq -r '[.[] | .createSurface? // empty] | first | .catalogId // empty')
if [ "$surface_catalog" = "$catalog_id" ]; then
  ok "run 1: createSurface names the screen's catalog ($catalog_id)"
else
  bad "run 1: createSurface names '${surface_catalog:-nothing}', want $catalog_id (operations: $(printf '%s' "$operations" | head -c 400))"
fi
components=$(printf '%s' "$operations" | jq -c '[.[] | .updateComponents? // empty | .components[]?]')
kinds=$(printf '%s' "$components" | jq -r 'map(.component) | join(" ")')
if printf '%s' "$components" | jq -e 'map(.component) | (index("Cards") != null) and (index("Mermaid") != null)' >/dev/null 2>&1; then
  ok "run 1: the surface holds a Cards and a Mermaid (components: $kinds)"
else
  bad "run 1: the surface holds '${kinds:-nothing}', want a Cards and a Mermaid (operations: $(printf '%s' "$operations" | head -c 400))"
fi
cards=$(printf '%s' "$components" | jq -c '[.[] | select(.component == "Cards")] | first // {}')
# shellcheck disable=SC2086 # $expected is a list of words (links) on purpose
expected_sorted=$(printf '%s\n' $expected | sort | tr '\n' ' ' | sed 's/ $//')
if [ "$(printf '%s' "$cards" | jq -r '(.cards // []) | map(.url) | sort | join(" ")')" = "$expected_sorted" ]; then
  ok "run 1: the Cards holds three cards, one for each link the search returned"
else
  bad "run 1: the Cards holds the links '$(printf '%s' "$cards" | jq -r '(.cards // []) | map(.url) | join(" ")')', want $expected"
fi
if printf '%s' "$cards" | jq -e '.cards | length == 3 and all(.[]; (.title | length > 0) and (.url | startswith("https://")))' >/dev/null 2>&1; then
  ok "run 1: each card has a title and an https link"
else
  bad "run 1: the cards lack a title or an https link: $(printf '%s' "$cards" | head -c 400)"
fi
if printf '%s' "$components" | jq -e '[.[] | select(.component == "Mermaid")] | first | .code | startswith("graph TD")' >/dev/null 2>&1; then
  ok "run 1: the Mermaid is a graph TD"
else
  bad "run 1: the Mermaid is not a graph TD: $(printf '%s' "$components" | head -c 400)"
fi
if [ "$(held)" = "$catalog_id $version $digest" ]; then
  ok "run 1: the thread's state says it holds the screen's catalog (version $version)"
else
  bad "run 1: thread.uiCatalog is '$(held)', want $catalog_id $version $digest"
fi
calls=$(curl -s --max-time 30 "$search/__journal" || true)
count=$(printf '%s' "$calls" | jq -r '.calls | length' 2>/dev/null || echo '?')
if [ "$count" = 1 ]; then
  ok "run 1: mock-mcp-search got exactly one call"
else
  bad "run 1: mock-mcp-search got $count calls, want exactly one ($(printf '%s' "$calls" | jq -c '.calls' 2>/dev/null || echo "$calls"))"
fi
query=$(printf '%s' "$calls" | jq -r '.calls[0].arguments.query // empty' 2>/dev/null || true)
case $query in
  *async*) ok "run 1: the search was web_search for \"$query\"" ;;
  *) bad "run 1: the search query was '${query:-none}', want one that holds async" ;;
esac
reqs=$(requests)
offered=$(printf '%s' "$reqs" | jq -r '[.[0].body.tools // [] | .[].function.name] | join(" ")')
for tool in search__web_search ui_catalog show; do
  case " $offered " in
    *" $tool "*) ok "run 1: the model was offered $tool" ;;
    *) bad "run 1: the model was not offered $tool (tools: ${offered:-none})" ;;
  esac
done
# One catalog tool: the thread-tools endpoint's get_ui_catalog is hidden from the model, which has ui_catalog (adam-rs ADR 0006
# status note, d56dd94). So the tool list no longer shows that the grant reached the agent; its log does: adam warns "the
# thread tools could not be listed" when it holds a grant it cannot use (plain http without MCP_ALLOW_INSECURE, an endpoint
# that does not answer).
case " $offered " in
  *" get_ui_catalog "*) bad "run 1: the model was offered get_ui_catalog beside ui_catalog (tools: $offered)" ;;
  *) ok "run 1: the model was not offered get_ui_catalog (one catalog tool, ui_catalog)" ;;
esac
if command -v docker >/dev/null 2>&1; then
  if docker compose -f "$root/compose.yaml" --profile app logs --no-color --since "$started" researcher 2>&1 | grep -q 'the thread tools could not be listed'; then
    bad "run 1: the researcher could not list the thread tools: is MCP_ALLOW_INSECURE set on it, and thread-tools in ORCH_SURFACES?"
  else
    ok "run 1: the researcher listed the thread tools (no warning in its log since $started)"
  fi
else
  echo "skip run 1: docker is not available, so the researcher's log was not read for the thread-tools grant"
fi
case $(tool_result "$reqs" cards-call-1) in
  *"$(printf '%s' "$expected" | cut -d ' ' -f 1)"*) ok "run 1: the results of the search went back to the model" ;;
  *) bad "run 1: the model got no search result for cards-call-1 ('$(tool_result "$reqs" cards-call-1 | head -c 200)')" ;;
esac
result=$(tool_result "$reqs" cards-call-2)
case $result in
  *Cards*Mermaid* | *Mermaid*Cards*) ok "run 1: ui_catalog told the model the screen has Cards and Mermaid" ;;
  *) bad "run 1: the result of ui_catalog (cards-call-2) names no Cards and Mermaid: '$(printf '%s' "$result" | head -c 300)'" ;;
esac
case $(tool_result "$reqs" cards-call-3) in
  *"Shown to the person"*) ok "run 1: show was accepted (\"Shown to the person.\")" ;;
  *) bad "run 1: the result of show (cards-call-3) is '$(tool_result "$reqs" cards-call-3 | head -c 300)', want \"Shown to the person.\"" ;;
esac
logged=$(catalogs_in_log)
if [ "$(printf '%s' "$logged" | jq -r 'length' 2>/dev/null)" = 1 ]; then
  ok "run 1: the thread's log holds one ui_catalog event"
else
  bad "run 1: the thread's log holds '$(printf '%s' "$logged" | jq -c 'map(.version)' 2>/dev/null || echo "$logged")' ui_catalog events, want one"
fi

# --- RUN 2: an older screen writes to the same thread --------------------------------------------------------------
echo
echo "run 2: the same thread from an older screen (it sends no catalog)"
input_message '[mock:cards] and one more thing about it' > "$tmp/run2.json"
stream "$tmp/run2.json" "run 2"
case $outcome in
  success | interrupt) ok "run 2: the run stream ended with RUN_FINISHED ($outcome)" ;;
  *) bad "run 2: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED" ;;
esac
case $state in
  done | blocked) ok "run 2: the thread ended $state, not failed" ;;
  *) bad "run 2: the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"; why "$events" ;;
esac
missing=
for url in $expected; do
  case $said in
    *"$url"*) ;;
    *) missing="$missing $url" ;;
  esac
done
if [ -z "$missing" ]; then ok "run 2: the words name the three links"; else bad "run 2: the words do not name$missing"; fi
logged=$(catalogs_in_log)
if [ "$(printf '%s' "$logged" | jq -r 'length' 2>/dev/null)" = 1 ]; then
  ok "run 2: the thread's log still holds one ui_catalog event: the older screen added none"
else
  bad "run 2: the thread's log holds '$(printf '%s' "$logged" | jq -c 'map(.version)' 2>/dev/null || echo "$logged")' ui_catalog events, want one"
fi
if [ "$(held)" = "$catalog_id $version $digest" ]; then
  ok "run 2: the thread's catalog stays version $version"
else
  bad "run 2: thread.uiCatalog is '$(held)', want $catalog_id $version $digest"
fi

# --- RUN 3: a screen whose catalog has no Cards -----------------------------------------------------------------------
thread=$(uuid)
echo
echo "run 3: a new thread from a screen on catalog version $old_version (no Cards, no Mermaid)"
echo "thread $thread"
input_message '[mock:cards] what is async rust?' "$old_version" "$old_digest" "$tmp/old.json" > "$tmp/run3.json"
stream "$tmp/run3.json" "run 3"
if [ "$outcome" = success ]; then
  ok "run 3: the run stream ended with RUN_FINISHED (success)"
else
  bad "run 3: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"
fi
if [ "$state" = "done" ]; then
  ok "run 3: the thread ended done"
else
  bad "run 3: the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"
  why "$events"
fi
for url in $expected; do
  case $said in
    *"$url"*) ok "run 3: the words name $url" ;;
    *) bad "run 3: the words do not name $url" ;;
  esac
done
surfaces=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface") | .messageId] | unique | length' "$events" 2>/dev/null || echo '?')
if [ "$surfaces" = 0 ]; then
  ok "run 3: no a2ui-surface: the screen cannot draw cards, the answer is words"
else
  bad "run 3: $surfaces a2ui-surface activities for a screen without Cards, want none"
fi
if [ "$(held)" = "$catalog_id $old_version $old_digest" ]; then
  ok "run 3: the thread's state holds the older screen's catalog (version $old_version)"
else
  bad "run 3: thread.uiCatalog is '$(held)', want $catalog_id $old_version $old_digest"
fi
reqs=$(requests)
result=$(printf '%s' "$reqs" | jq -r '[.[] | .body.messages // [] | .[] | select(.role == "tool" and .tool_call_id == "cards-call-3") | .content] | last // empty')
case $result in
  "") bad "run 3: the model got no result for its show call (cards-call-3)" ;;
  *"Shown to the person"*) bad "run 3: show was accepted for a screen whose catalog has no Cards: '$result'" ;;
  *) ok "run 3: show was refused for a screen without Cards, and the researcher answered in words anyway" ;;
esac

# --- what the researcher's model saw ----------------------------------------------------------------------------------
echo
total=$(printf '%s' "$reqs" | jq -r 'length')
errors=$(printf '%s' "$reqs" | jq -r '[.[] | select(.status != 200)] | length')
if [ "$total" -ge 9 ] && [ "$errors" = 0 ]; then
  ok "the researcher's model answered all $total requests of the three runs without an error"
else
  bad "the model mock got $total mock-researcher requests, $errors of them not answered 200 (want at least 9 and no error)"
fi
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then
  ok "mock-model matched every request"
else
  bad "mock-model saw $unmatched unmatched requests (see $model/__admin/requests/unmatched)"
fi

finish
