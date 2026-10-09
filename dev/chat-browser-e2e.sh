#!/usr/bin/env sh
# System-level test of the chat's remote sub-agent `browser` (ADR 0057, amended 2026-10-09; adam-rs ADR 0033): what the chart renders
# with `browser.chatSubagent: true`. The chat's folder has a sub-agent file with `a2a:` the browser's card, `auth: bearer:BROWSER_A2A_TOKEN`
# and `files: true` (dev/agents/browser/chat-subagent.md, the chart's files/browser/chat-subagent.md), so the chat's model has a tool
# `browser`; a call is a task of the browser agent (adam-agent beside obscura), whose screenshot is a file of the browser's run (obscura's
# entry says `files: true`), and comes back with the answer as a file of the CHAT's run, which the orchestrator keeps in its artifact store.
# No mention and no `ask_agent`: the chat asks the browser itself, over plain http inside the compose network (A2A_ALLOW_INSECURE_REMOTES).
# The models are scripts: `[mock:browse] [mock:browser-tool]` of `mock-persona` (dev/wiremock/model/mappings/browse-subagent*.json) and the
# browser's own `mock-browse` (browser*.json); the browser, obscura and the page are real.
#
#   docker compose -f compose.yaml -f dev/compose.chat-browser.yaml --profile app up -d --build --wait
#   dev/chat-browser-e2e.sh
#
# Without the override the chat has no tool `browser`, and the script stops at the first check that says so.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/agents lists `chat` and `browser`;
#   * the person's message to the chat names the page and asks to see it, mentioning nobody: the run ends RUN_FINISHED (success) and the
#     thread `done` (the chat reads the remote task every 60 s, adam's default, so the run takes about a minute);
#   * the chat's model was offered `browser` (its sub-agent) and no `ask_agent` (nobody was mentioned); two requests, the second ending with
#     the result of `browser-sub-1`: the browser's words (the page's code and count) and the line of the file shared in the chat's run,
#     "Shared browser_screenshot-<hash>-<hash>.png (<size>, image/png). To show it in your answer, write ...";
#   * the browser's model (`mock-browse`) got five requests, the first the chat's message, the fifth the result of its screenshot, shared in
#     the browser's run ("Shared browser_screenshot-<hash>.png");
#   * the chat thread's log holds exactly one artifact with a file: an image/png named browser_screenshot-<hash>-<hash>.png (the name the
#     chat's run made from the browser's), of a nonzero size, attributed to the chat, a reference with no bytes; no ask_started (the chat
#     asked its own sub-agent, not the orchestrator);
#   * the API serves it to the owner as a PNG of that size; the run's AG-UI frames carry it (`vymalo.artifact`, the same sha256);
#   * the chat's final message names the page's code, count and URL and shows the screenshot by its file name;
#   * `mock-model` matched every request.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL   http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   PAGE_URL         http://browser-site:8080/   the page, as the browser's sidecar reaches it on the compose network
#   TIMEOUT          240    seconds to wait for the run (the chat's first look at the remote task is 60 s after it asks)
#
# It EMPTIES the request journal of `mock-model` first, so run it on a stack you are not in the middle of another scenario on. Needs curl and
# jq (and /proc or uuidgen for a UUID).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
id_header=$(sh "$here/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
page=${PAGE_URL:-http://browser-site:8080/}
timeout=${TIMEOUT:-240}

code='Lighthouse code: LH-7731-QUILL'
count='Ships counted in October: 42'

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "chat-browser e2e passed"; else echo "chat-browser e2e FAILED"; exit 1; fi
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

sse_events() { sed -n 's/^data: *//p' "$1"; }

# run_agui AGENT THREAD TEXT: one run (a message) on the thread, to its end. Prints the HTTP status; the body is $tmp/run.sse.
run_agui() {
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

run_outcome() { # run_outcome FILE: how the run saved in FILE ended
  sse_events "$1" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true
}

wait_state() { # wait_state THREAD STATE...: the state the thread ended in (or the last one seen after TIMEOUT seconds)
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

model_requests() { # model_requests NAME: the bodies of the chat completions the model mock got for NAME, the oldest first
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c --arg m "$1" '[.requests | reverse | .[].request.body | fromjson? | select(.model == $m)]' 2>/dev/null || echo '[]'
}

# --- the stack, and the agents the scenario needs -------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
for a in chat browser; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done
ok "GET /api/agents lists chat and browser"

code_reset=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
expect "the journal of mock-model is emptied" "$code_reset" "200"

# --- the person asks the chat; nobody is mentioned ---------------------------------------------------------------------
thread=$(uuid)
echo "== thread $thread (chat), the chat's own sub-agent browser"
code_run=$(run_agui chat "$thread" "[mock:browse] [mock:browser-tool] [mock:shot] What does $page look like? Ask the browser and show me.")
if [ "$code_run" != 200 ]; then
  bad "POST /agui/agents/chat answered HTTP ${code_run:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
cp "$tmp/run.sse" "$tmp/main.run.sse"
sse_events "$tmp/main.run.sse" | jq -s '.' >"$tmp/live.json" 2>/dev/null || echo '[]' >"$tmp/live.json"

echo "== what the chat's model was sent (mock-persona)"
model_requests mock-persona >"$tmp/persona.json"
offered=$(jq -r '[.[0].tools // [] | .[].function.name] | join(" ")' "$tmp/persona.json")
case " $offered " in
  *" browser "*) ok "the chat's model was offered its sub-agent browser" ;;
  *)
    bad "the chat's model was not offered browser (tools: ${offered:-none}): is the stack up with -f dev/compose.chat-browser.yaml?"
    finish
    ;;
esac
case " $offered " in
  *" ask_agent "*) bad "the chat's model was offered ask_agent, but nobody was mentioned (tools: $offered)" ;;
  *) ok "and no ask_agent: nobody was mentioned, the chat asks the browser itself" ;;
esac
expect "the run stream ended with RUN_FINISHED (success)" "$(run_outcome "$tmp/main.run.sse")" "success"
expect "the thread ended done" "$(wait_state "$thread" "done" blocked)" "done"
model_requests mock-persona >"$tmp/persona.json"
expect "two model requests: the call of browser and the answer" "$(jq -r 'length' "$tmp/persona.json")" "2"
expect "the second ends with the result of browser-sub-1, which holds the page's code" \
  "$(jq -r --arg c "$code" '.[1].messages[-1] | "\(.tool_call_id):\(.content | tostring | contains($c))"' "$tmp/persona.json")" "browser-sub-1:true"
expect "and the line of the screenshot, shared as a file of the chat's run (browser_screenshot-<hash>-<hash>.png)" \
  "$(jq -r '.[1].messages[-1].content | tostring | test("Shared browser_screenshot-[0-9a-f]{8}-[0-9a-f]{8}[.]png [(][^)]*, image/png[)][.] To show it in your answer, write !\\[")' "$tmp/persona.json")" "true"

echo "== what the browser's model was sent (mock-browse)"
model_requests mock-browse >"$tmp/browse.json"
expect "five model requests: the chat's message, then the results of close, navigate, markdown and screenshot" "$(jq -r 'length' "$tmp/browse.json")" "5"
expect "the first ends with the chat's message: open the page, take a screenshot" \
  "$(jq -r '.[0].messages[-1] | "\(.role):\(.content | tostring | (startswith("Open '"$page"',") and contains("Take a screenshot")))"' "$tmp/browse.json")" "user:true"
expect "the fifth ends with the screenshot, shared as a file of the browser's run" \
  "$(jq -r '.[4].messages[-1].content | tostring | test("^Shared browser_screenshot-[0-9a-f]{8}[.]png ")' "$tmp/browse.json")" "true"

echo "== the chat thread's log (the export)"
api GET "/api/threads/$thread/export" >"$tmp/export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/export.json"
expect "no ask_started: the chat asked its own sub-agent, not the orchestrator" \
  "$(jq -r '[.events[] | select(.kind == "ask_started")] | length' "$tmp/export.json")" "0"
files=$(jq -c '[.events[] | select(.kind == "artifact" and .data.file != null)]' "$tmp/export.json")
expect "one artifact with a file: an image/png named browser_screenshot-<hash>-<hash>.png, of a nonzero size, attributed to the chat" \
  "$(printf '%s' "$files" | jq -r 'map("\(.data.mimeType):\(.data.file.filename | test("^browser_screenshot-[0-9a-f]{8}-[0-9a-f]{8}[.]png$")):\(.data.file.size > 0):\(.actor.name)") | join(" ")')" \
  "image/png:true:true:chat"
expect "the artifact event is a reference, with no bytes, text or link" \
  "$(printf '%s' "$files" | jq -r 'all(.[]; .data.text == null and .data.uri == null and ((.data | tojson | length) < 600))')" "true"
shot_name=$(printf '%s' "$files" | jq -r '.[0].data.file.filename // ""')
shot_sha=$(printf '%s' "$files" | jq -r '.[0].data.file.sha256 // ""')
shot_size=$(printf '%s' "$files" | jq -r '.[0].data.file.size // 0')
curl -sS --max-time 30 -o "$tmp/shot.png" -H "$id_header" "$base/api/threads/$thread/artifacts/$shot_sha" 2>/dev/null || : >"$tmp/shot.png"
expect "the API serves it to the owner: a PNG of the size the log says" \
  "$(head -c 4 "$tmp/shot.png" | od -An -c | tr -d ' \n'):$(wc -c <"$tmp/shot.png" | tr -d ' ')" "211PNG:$shot_size"
expect "the run's AG-UI frames carry it (vymalo.artifact, the same sha256)" \
  "$(jq -r --arg sha "$shot_sha" '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact" and .content.sha256 == $sha)] | length > 0' "$tmp/live.json")" "true"

answer=$(jq -r '[.events[] | select(.kind == "agent_message" and .data.final == true) | .data.text] | last // empty' "$tmp/export.json")
echo "the chat said: ${answer:-<nothing>}"
case $answer in
  *"$code"*"$count"*"Source: $page"*) ok "the chat's final message names the page's code, its count and its URL" ;;
  *) bad "the chat's final message does not name '$code', '$count' and 'Source: $page'" ;;
esac
case $answer in
  *"](${shot_name:-none})"*) ok "and shows the screenshot by its file name ($shot_name)" ;;
  *) bad "the chat's final message does not show '](${shot_name:-none})'" ;;
esac

unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "mock-model matched every request" "$unmatched" "0"

finish
