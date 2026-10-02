#!/usr/bin/env sh
# System-level test of the coder asking with Choices (MVP slice 3, ADR 0023): the screen's UI catalog goes to the
# coder with the first message, the coder asks three questions at once as ONE form drawn from that catalog, the
# person answers with one A2UI action, and the coder goes on with what was chosen. A second run with a newer
# catalog shows a thread that is opened in two versions of the screen records both.
#
#   dev/choices-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the web does (docs/api/agui.md): one POST /agui/agents/coder per run, the thread's
# state from GET /api/threads/{id}, its frames from GET /agui/threads/{id}/connect?mode=run, its log from
# GET /api/threads/{id}/export. The catalog it sends is the one the web ships, read from
# web/src/features/chat/lib/a2ui/catalog/catalog.json and catalog.lock.json (`forwardedProps["vymalo.uiCatalog"]`).
# The coder's model is `mock-coder`: a task that carries `[mock:choices]` makes it call `ask_user` with three
# questions (database, login, where it runs), and it answers the person's choices with "Going with Postgres, Keycloak
# and Compose." (dev/coder/wiremock/mock-openai/mappings/coder-choices.json, vendored from adam-rs). No repository,
# GitHub or OpenCode is involved.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the screen's catalog is what its lock says (the digest below is computed here with jq and sha256sum, as the
#     second run needs), the coder is listed by GET /api/agents and its card lists A2UI v0.9.1 (with inline catalogs),
#     ui-catalog/v1 and thread-tools/v1;
#   * RUN 1, "[mock:choices] set up the project" with the catalog: the run ends RUN_FINISHED (interrupt) and the
#     thread `blocked`; the coder's words are the question; there is exactly one `a2ui-surface`, which names the
#     screen's catalogId (createSurface) and holds one `Choices` of three questions (db, auth, deploy); the state of the
#     thread says which catalog it holds (`thread.uiCatalog`); the model of the coder was offered `ask_user` with a
#     `choices` parameter, `show` and `ui_catalog`, not `get_ui_catalog` (one catalog tool), and the coder's log says it
#     listed the thread tools (the grant reached it over plain http, which needs MCP_ALLOW_INSECURE on the coder);
#   * RUN 2, the answers (db=pg, auth=keycloak, deploy=compose) as ONE `forwardedProps.a2uiAction.userAction`: the thread
#     records a `vymalo.action` that carries them, and the coder's next words quote "Postgres", "Keycloak" and "Compose"
#     (the mock says that only when the tool result it was given holds `db: pg`); the model got all three answers as
#     the result of its `ask_user` call;
#   * RUN 3, a message with a newer catalog (the shipped one plus a dummy component, version + 1, digest recomputed):
#     the thread's log records a SECOND `ui_catalog` (the first is the shipped one, unchanged), the state says the
#     newer one is the thread's, the run did not fail, and the coder's model mock answered every request (no error, none
#     unmatched).
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`, which injects the identity
#   AUTH_EMAIL       dev@example.com, sent as X-Auth-Request-Email (the edge replaces it)
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}, the coder's model
#   CODER_URL        http://127.0.0.1:${CODER_PORT:-8090}, the coder itself, only to read its card
#   CATALOG_FILE     web/src/features/chat/lib/a2ui/catalog/catalog.json        the screen's catalog
#   CATALOG_LOCK     web/src/features/chat/lib/a2ui/catalog/catalog.lock.json   its {version, digest}
#   TIMEOUT          120    seconds to wait for a run to end
#
# It EMPTIES the request journal of `mock-openai` first, so run it on a stack you are not in the middle of another
# scenario on. Needs curl, jq and sha256sum (or shasum), and /proc or uuidgen for a UUID. Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
openai=${MOCK_OPENAI_URL:-http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}}
openai=${openai%/}
coder=${CODER_URL:-http://127.0.0.1:${CODER_PORT:-8090}}
coder=${coder%/}
timeout=${TIMEOUT:-120}

root=$(cd "$(dirname "$0")/.." && pwd)
# when this run began: the agent's log is read from here on (grant_reached)
started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
catalog_file=${CATALOG_FILE:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.json}
catalog_lock=${CATALOG_LOCK:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.lock.json}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "choices e2e passed"; else echo "choices e2e FAILED"; exit 1; fi
}

for f in "$catalog_file" "$catalog_lock"; do
  if [ ! -f "$f" ]; then
    echo "FAIL $f does not exist (CATALOG_FILE and CATALOG_LOCK name the screen's catalog and its lock)"
    exit 1
  fi
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "X-Auth-Request-Email: $email"
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

# --- the screen's catalog --------------------------------------------------------------------------------
catalog_id=$(jq -r '.catalogId' "$catalog_file")
version=$(jq -r '.version' "$catalog_lock")
digest=$(jq -r '.digest' "$catalog_lock")
echo "the screen's catalog: $catalog_id, version $version, $digest"
computed=$(canonical_digest "$catalog_file")
if [ "$computed" = "$digest" ]; then
  ok "the lock matches the catalog (the digest computed here with jq and sha256sum is the lock's)"
  digest_ok=1
else
  bad "the lock says $digest, the catalog's canonical form hashes to $computed here (is catalog.lock.json out of date, or is this jq's canonical form not the web's?)"
  digest_ok=0
fi
# The newer catalog of run 3: the shipped one plus a component of no use, one version up.
newer_version=$((version + 1))
jq '.components.Dummy = {type: "object", properties: {component: {const: "Dummy"}, text: {type: "string", maxLength: 10}}, required: ["component", "text"]}' \
  "$catalog_file" > "$tmp/newer.json"
newer_digest=$(canonical_digest "$tmp/newer.json")

# --- the agent and its card -------------------------------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  if printf '%s' "$agents" | jq -e 'any(.[]; .id == "coder")' >/dev/null 2>&1; then
    ok "GET /api/agents lists coder"
  else
    bad "GET /api/agents does not list coder (agents: $(printf '%s' "$agents" | jq -c '[.[].id]'))"
    finish
  fi
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi
a2ui_uri=https://a2ui.org/a2a-extension/a2ui/v0.9.1
ui_uri=https://agents.vymalo.com/a2a/extensions/ui-catalog/v1
tools_uri=https://agents.vymalo.com/a2a/extensions/thread-tools/v1
code=$(curl -s -o "$tmp/card.json" -w '%{http_code}' --max-time 30 "$coder/.well-known/agent-card.json" || true)
if [ "$code" != 200 ]; then
  bad "the coder's card: GET $coder/.well-known/agent-card.json answered HTTP ${code:-none} (is the coder's port published? CODER_URL names it)"
else
  for uri in "$a2ui_uri" "$ui_uri" "$tools_uri"; do
    if jq -e --arg u "$uri" '.capabilities.extensions[]? | select(.uri == $u)' "$tmp/card.json" >/dev/null 2>&1; then
      ok "the coder's card lists $uri"
    else
      bad "the coder's card does not list $uri (is the image pinned in compose.yaml an adam-rs commit with ask-with-choices, d411249 or later?)"
    fi
  done
  if jq -e --arg u "$a2ui_uri" '.capabilities.extensions[]? | select(.uri == $u) | .params.acceptsInlineCatalogs == true' "$tmp/card.json" >/dev/null 2>&1; then
    ok "the A2UI entry of the card accepts inline catalogs"
  else
    bad "the A2UI entry of the card does not say acceptsInlineCatalogs: true"
  fi
fi

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$openai/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $openai"; else bad "journal reset: $openai answered HTTP $code"; fi

# --- one run ------------------------------------------------------------------------------------------------
thread=$(uuid)
echo "thread $thread"

# stream INPUT_FILE LABEL: POST the RunAgentInput to the coder and wait for the run to end. Sets
#   outcome   how the run stream ended (success, interrupt, error: <code>, or empty)
#   state     the state the thread ended in
#   said      the words the coder spoke in this run (the assistant messages of its stream), joined
#   events    the file holding every frame of the thread so far (a JSON array)
stream() {
  _deadline=$(( $(date +%s) + timeout ))
  _code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/coder" -H "X-Auth-Request-Email: $email" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary "@$1" 2>"$tmp/err" || true)
  if [ "$_code" != 200 ]; then
    bad "$2: POST /agui/agents/coder answered HTTP ${_code:-none}: $(head -c 300 "$tmp/err") $(head -c 400 "$tmp/run.sse" 2>/dev/null)"
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
  state=
  while :; do
    state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
    case $state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 2
  done
  events=$tmp/events.json
  curl -sS --max-time 60 -H "X-Auth-Request-Email: $email" -H 'accept: text/event-stream' \
    "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
    echo '[]' > "$events"
  echo "$2: the coder said: ${said:-<nothing>}"
}

# why EVENTS: what the thread said about a failure, to help whoever reads the log.
why() {
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$1" | head -n 20
}

# input_message TEXT VERSION DIGEST CATALOG_FILE: the RunAgentInput of a run with one new user message and the catalog
# the screen sends under forwardedProps["vymalo.uiCatalog"] (the file is read as jq's input, so `.` is the catalog).
input_message() {
  jq -c --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" \
    --argjson version "$2" --arg digest "$3" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [],
      messages: [{id: $msg, role: "user", content: $text}],
      forwardedProps: {"vymalo.uiCatalog": {catalogId: .catalogId, version: $version, digest: $digest, catalog: .}}}' "$4"
}

# requests: the bodies and statuses of the requests the coder's model mock got for mock-coder, oldest first
# (WireMock's journal lists the newest first), as a JSON array of {status, body}.
requests() {
  curl -s --max-time 30 "$openai/__admin/requests" |
    jq -c '[.requests | reverse | .[] | {status: .response.status, body: (.request.body | fromjson? // {})} | select(.body.model == "mock-coder")]' 2>/dev/null || echo '[]'
}

# --- RUN 1: three questions at once, as one form ---------------------------------------------------------------
echo
echo "run 1: [mock:choices] with the screen's catalog (version $version)"
input_message '[mock:choices] set up the project' "$version" "$digest" "$catalog_file" > "$tmp/run1.json"
stream "$tmp/run1.json" "run 1"
if [ "$outcome" = interrupt ]; then
  ok "run 1: the run stream ended with RUN_FINISHED (interrupt): the coder waits for the person"
else
  bad "run 1: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (interrupt)"
fi
if [ "$state" = blocked ]; then
  ok "run 1: the thread ended blocked (the coder asked)"
else
  bad "run 1: the thread ended '${state:-unknown}' (after at most ${timeout}s), want blocked"
  why "$events"
fi
case $said in
  *"Three quick questions before I start"*) ok "run 1: the question is the coder's text" ;;
  *) bad "run 1: the coder's words do not hold the question \"Three quick questions before I start\"" ;;
esac
surfaces=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface") | .messageId] | unique | length' "$events" 2>/dev/null || echo '?')
if [ "$surfaces" = 1 ]; then
  ok "run 1: exactly one a2ui-surface"
else
  bad "run 1: $surfaces a2ui-surface activities, want exactly one"
fi
# The last snapshot of the surface holds all of it (replace: true).
operations=$(jq -c '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface")] | last | .content.a2ui_operations // []' "$events" 2>/dev/null || echo '[]')
surface_catalog=$(printf '%s' "$operations" | jq -r '[.[] | .createSurface? // empty] | first | .catalogId // empty')
surface_id=$(printf '%s' "$operations" | jq -r '[.[] | .createSurface? // empty] | first | .surfaceId // empty')
if [ "$surface_catalog" = "$catalog_id" ]; then
  ok "run 1: createSurface names the screen's catalog ($catalog_id)"
else
  bad "run 1: createSurface names '${surface_catalog:-nothing}', want $catalog_id (operations: $(printf '%s' "$operations" | head -c 400))"
fi
choices=$(printf '%s' "$operations" | jq -c '[.[] | .updateComponents? // empty | .components[]? | select(.component == "Choices")]')
if [ "$(printf '%s' "$choices" | jq -r 'length')" = 1 ]; then
  ok "run 1: the surface has one Choices"
else
  bad "run 1: the surface has $(printf '%s' "$choices" | jq -r 'length') Choices components, want one (operations: $(printf '%s' "$operations" | head -c 400))"
fi
if printf '%s' "$choices" | jq -e '.[0] | (.questions | length) == 3 and (.questions | map(.id)) == ["db", "auth", "deploy"]' >/dev/null 2>&1; then
  ok "run 1: the Choices has three questions: db, auth, deploy"
else
  bad "run 1: the Choices does not hold the three questions db, auth, deploy: $(printf '%s' "$choices" | head -c 400)"
fi
choices_id=$(printf '%s' "$choices" | jq -r '.[0].id // empty')
event_name=$(printf '%s' "$choices" | jq -r '.[0].action.event.name // "answer"')
held=$(jq -c '[.[] | select(.type == "STATE_SNAPSHOT") | .snapshot.thread.uiCatalog // empty] | last // {}' "$events" 2>/dev/null || echo '{}')
if [ "$(printf '%s' "$held" | jq -r '[.catalogId, (.version | tostring), .digest] | join(" ")')" = "$catalog_id $version $digest" ]; then
  ok "run 1: the thread's state says it holds the screen's catalog (version $version)"
else
  bad "run 1: thread.uiCatalog is $held, want $catalog_id version $version $digest"
fi
reqs=$(requests)
offered=$(printf '%s' "$reqs" | jq -r '[.[0].body.tools // [] | .[].function.name] | join(" ")')
for tool in ask_user show ui_catalog; do
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
  if docker compose -f "$root/compose.yaml" --profile app logs --no-color --since "$started" coder 2>&1 | grep -q 'the thread tools could not be listed'; then
    bad "run 1: the coder could not list the thread tools: is MCP_ALLOW_INSECURE set on it, and thread-tools in ORCH_SURFACES?"
  else
    ok "run 1: the coder listed the thread tools (no warning in its log since $started)"
  fi
else
  echo "skip run 1: docker is not available, so the coder's log was not read for the thread-tools grant"
fi
if printf '%s' "$reqs" | jq -e '.[0].body.tools // [] | map(select(.function.name == "ask_user")) | .[0].function.parameters.properties | has("choices")' >/dev/null 2>&1; then
  ok "run 1: ask_user takes a choices parameter"
else
  bad "run 1: ask_user has no choices parameter in the tools the model was offered"
fi

# --- RUN 2: the answers, as one A2UI action -----------------------------------------------------------------
echo
echo "run 2: the answers db=pg, auth=keycloak, deploy=compose as one action"
if [ -z "$surface_id" ] || [ -z "$choices_id" ]; then
  bad "run 2: run 1 gave no surface or no Choices component to answer: the answer cannot be sent"
  finish
fi
jq -nc --arg thread "$thread" --arg run "$(uuid)" --arg surface "$surface_id" --arg source "$choices_id" --arg name "$event_name" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [], messages: [],
    forwardedProps: {a2uiAction: {userAction: {
      name: $name, surfaceId: $surface, sourceComponentId: $source, timestamp: $ts,
      context: {answers: [
        {id: "db", values: ["pg"]}, {id: "auth", values: ["keycloak"]}, {id: "deploy", values: ["compose"]}]}}}}}' > "$tmp/run2.json"
stream "$tmp/run2.json" "run 2"
case $outcome in
  interrupt | success) ok "run 2: the run stream ended with RUN_FINISHED ($outcome)" ;;
  *) bad "run 2: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED" ;;
esac
case $state in
  blocked | done) ok "run 2: the thread ended $state, not failed" ;;
  *) bad "run 2: the thread ended '${state:-unknown}' (after at most ${timeout}s), want blocked or done"; why "$events" ;;
esac
recorded=$(jq -c '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.action")] | last | .content // {}' "$events" 2>/dev/null || echo '{}')
if printf '%s' "$recorded" | jq -e --arg s "$surface_id" '.surfaceId == $s and (.context.answers | map("\(.id)=\(.values | join(","))")) == ["db=pg", "auth=keycloak", "deploy=compose"]' >/dev/null 2>&1; then
  ok "run 2: the thread recorded one vymalo.action with the three answers, on the surface the coder drew"
else
  bad "run 2: the thread's vymalo.action is $recorded, want the three answers on surface $surface_id"
fi
for word in Postgres Keycloak Compose; do
  case $said in
    *"$word"*) ok "run 2: the coder's next words quote $word" ;;
    *) bad "run 2: the coder's next words do not quote $word" ;;
  esac
done
reqs=$(requests)
result=$(printf '%s' "$reqs" | jq -r '[.[] | .body.messages // [] | .[] | select(.role == "tool" and .tool_call_id == "choices-call-1") | .content] | last // empty')
case $result in
  *"db: pg"*"auth: keycloak"*"deploy: compose"*) ok "run 2: the model got the three answers as the result of its ask_user call" ;;
  *) bad "run 2: the result of the ask_user call (choices-call-1) was '${result:-none}', want db: pg, auth: keycloak, deploy: compose" ;;
esac

# --- RUN 3: a newer screen ----------------------------------------------------------------------------------
echo
echo "run 3: a message from a newer screen (catalog version $newer_version, one more component)"
input_message '[mock:choices] thanks, and one more thing' "$newer_version" "$newer_digest" "$tmp/newer.json" > "$tmp/run3.json"
stream "$tmp/run3.json" "run 3"
case $outcome in
  interrupt | success) ok "run 3: the run stream ended with RUN_FINISHED ($outcome)" ;;
  *) bad "run 3: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED" ;;
esac
case $state in
  blocked | done) ok "run 3: the thread ended $state, not failed" ;;
  *) bad "run 3: the thread ended '${state:-unknown}' (after at most ${timeout}s), want blocked or done"; why "$events" ;;
esac
held=$(jq -c '[.[] | select(.type == "STATE_SNAPSHOT") | .snapshot.thread.uiCatalog // empty] | last // {}' "$events" 2>/dev/null || echo '{}')
if [ "$(printf '%s' "$held" | jq -r '[.catalogId, (.version | tostring), .digest] | join(" ")')" = "$catalog_id $newer_version $newer_digest" ]; then
  ok "run 3: the thread's state now holds the newer catalog (version $newer_version)"
else
  bad "run 3: thread.uiCatalog is $held, want $catalog_id version $newer_version $newer_digest"
fi
if api GET "/api/threads/$thread/export" > "$tmp/export.json" 2>"$tmp/err"; then
  catalogs=$(jq -c '[.events[] | select(.kind == "ui_catalog") | .data]' "$tmp/export.json")
  if [ "$(printf '%s' "$catalogs" | jq -r 'length')" = 2 ]; then
    ok "run 3: the thread's log holds two ui_catalog events"
  else
    bad "run 3: the thread's log holds $(printf '%s' "$catalogs" | jq -r 'length') ui_catalog events, want two"
  fi
  if printf '%s' "$catalogs" | jq -e --argjson v "$version" --arg d "$digest" '.[0] | .version == $v and .digest == $d' >/dev/null 2>&1; then
    ok "run 3: the first is the shipped catalog (version $version), unchanged"
  else
    bad "run 3: the first ui_catalog is not the shipped catalog: $(printf '%s' "$catalogs" | jq -c '[.[] | {version, digest}]')"
  fi
  if printf '%s' "$catalogs" | jq -e --argjson v "$newer_version" --arg d "$newer_digest" '.[1] | .version == $v and .digest == $d and (.catalog.components | has("Dummy"))' >/dev/null 2>&1; then
    ok "run 3: the second is the newer catalog (version $newer_version, with its extra component)"
  else
    bad "run 3: the second ui_catalog is not the newer catalog: $(printf '%s' "$catalogs" | jq -c '[.[] | {version, digest}]')"
  fi
else
  bad "run 3: GET /api/threads/$thread/export: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/export.json")"
fi

# --- what the coder's model saw -----------------------------------------------------------------------------
echo
reqs=$(requests)
total=$(printf '%s' "$reqs" | jq -r 'length')
errors=$(printf '%s' "$reqs" | jq -r '[.[] | select(.status != 200)] | length')
if [ "$total" -ge 3 ] && [ "$errors" = 0 ]; then
  ok "the coder's model answered all $total requests of the three runs without an error"
else
  bad "the coder's model mock got $total mock-coder requests, $errors of them not answered 200 (want at least 3 and no error)"
fi
unmatched=$(curl -s --max-time 30 "$openai/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then
  ok "mock-openai matched every request"
else
  bad "mock-openai saw $unmatched unmatched requests (see $openai/__admin/requests/unmatched)"
fi
if [ "$digest_ok" = 0 ]; then
  echo "     (the digests sent were the lock's and the one computed here, which disagree: see the first FAIL)"
fi

finish
