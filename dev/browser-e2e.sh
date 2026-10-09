#!/usr/bin/env sh
# System-level test of the browser agent (ADR 0057): a person mentions @browser to the chat, the chat's model asks the browser with
# `ask_agent` (ADR 0026), and the browser, a folder served by adam-agent beside obscura (a headless browser with an MCP server, its
# sidecar), opens a page, reads it, takes a screenshot and answers; the chat names what the page says. The page is `browser-site`
# (dev/browser-site/index.html, a lighthouse log): the code and the count the scenario looks for are written there and nowhere else, so
# an answer that holds them is one the browser read through obscura. The models are scripts (dev/wiremock/model/mappings/browse*.json for
# the chat's `[mock:browse]`, browser*.json for the browser's own `mock-browse`); the browser, obscura and the page are real.
#
#   dev/browser-e2e.sh
#
# Start the `app` profile first (the browser runs `adam-agent` from the coder's image, about 2.9 GB, linux/amd64 only; obscura is about
# 100 MB):
#
#   docker compose --profile app up -d --build --wait
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/agents lists `chat` and `browser`; the capabilities of chat list mentions/v1 and thread-tools/v1, and the browser's card is read
#     (its name, Browser);
#   * the person's message to the chat mentions @browser (the reference as sent is in the log) and asks to see the page (`[mock:shot]`): the run
#     ends with RUN_FINISHED (success) and the thread `done`;
#   * one `ask_started` by `main` at depth 1 on `browser` (step `ask-1`, attributed to the chat, its text the model's request: open the page the
#     person named), and its `ask_finished` is `completed`, attributed to the browser, with the page's code and count and its URL;
#   * the chat's final message names the code, the count and the URL;
#   * what the browser's model was sent (`mock-browse`, from the model mock's journal): five requests, in order the ask's words, then the result
#     of `browser__browser_close` (each task starts from a clean browser), of `browser__browser_navigate` (the page's title, from obscura), of
#     `browser__browser_markdown` (the page's own words) and of `browser__browser_screenshot` (a PNG: today described to the model as
#     "[image not included: image/png]", adam-rs at the pinned revision includes no image bytes); the functions it was offered are the
#     browser's allow-listed tools (close, navigate, markdown, screenshot among them) and none of those the folder leaves out (evaluate,
#     cookies, storage state, the bulk form fill), and no ask_user or show;
#   * the AG-UI stream has `SUBAGENT_STARTED` `sub-ask-1` named browser under the chat's run, ending `completed`;
#   * obscura answers a request without its bearer with 401, from inside the browser's own network namespace (needs docker compose; SKIP
#     without it);
#   * `mock-model` matched every request.
#
# TODO(adam-rs, ADR 0057): adam-rs has, not yet merged on 2026-10-09, a per-server opt-in that turns an MCP image or PDF result into a file
# shared with the person. Once the pin is past it and the folder's mcp.json turns it on (its TODO key marks the place), run with
# BROWSER_SHARE_FILES=1: the screenshot must then reach the thread as a file (`ask_finished.artifacts` holds an image/png, the log a
# reference, and the API serves the PNG), and the browser's model is no longer told "[image not included]".
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL              http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL            dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL        http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   PAGE_URL              http://browser-site:8080/   the page, as the browser's sidecar reaches it on the compose network
#   BROWSER_SHARE_FILES   unset; 1 asserts the screenshot as a file of the thread (see the TODO above)
#   TIMEOUT               120    seconds to wait for a thread to stop
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
share_files=${BROWSER_SHARE_FILES:-}
timeout=${TIMEOUT:-120}

mentions_uri=https://agents.vymalo.com/a2a/extensions/mentions/v1
tools_uri=https://agents.vymalo.com/a2a/extensions/thread-tools/v1
code='Lighthouse code: LH-7731-QUILL'
count='Ships counted in October: 42'

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
skip() { echo "skip $1"; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "browser e2e passed"; else echo "browser e2e FAILED"; exit 1; fi
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

# run_agui AGENT THREAD TEXT FORWARDED_PROPS_JSON: one run (a message) on the thread, to its end. Prints the HTTP status; the body is $tmp/run.sse.
run_agui() {
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" --argjson props "$4" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
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
expect "the capabilities of chat list mentions/v1 and thread-tools/v1 (the person may mention the browser, ask_agent can reach the chat)" \
  "$(api GET /agui/agents/chat/capabilities 2>/dev/null | jq -r --arg m "$mentions_uri" --arg t "$tools_uri" '(.custom // {}) | [has($m), has($t)] | join(" ")')" "true true"
expect "the browser's card is read: its name is Browser" \
  "$(api GET /agui/agents/browser/capabilities 2>/dev/null | jq -r '.identity.name // "none"')" "Browser"

code_reset=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
expect "the journal of mock-model is emptied" "$code_reset" "200"

# --- the person asks the chat, mentioning @browser ---------------------------------------------------------------------
thread=$(uuid)
text="[mock:browse] [mock:shot] What does $page say? @browser please read it and show me."
before=${text%%@browser*}
mentions=$(jq -cn --argjson start "${#before}" '[{agentId: "browser", label: "@browser", start: $start, end: ($start + 8)}]')
echo "== thread $thread (chat), mentioning @browser"
code_run=$(run_agui chat "$thread" "$text" "$(jq -cn --argjson m "$mentions" '{"vymalo.mentions": $m}')")
if [ "$code_run" != 200 ]; then
  bad "POST /agui/agents/chat answered HTTP ${code_run:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
cp "$tmp/run.sse" "$tmp/main.run.sse"
expect "the run stream ended with RUN_FINISHED (success)" "$(run_outcome "$tmp/main.run.sse")" "success"
expect "the thread ended done" "$(wait_state "$thread" "done" blocked)" "done"
api GET "/api/threads/$thread/export" >"$tmp/export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/export.json"
sse_events "$tmp/main.run.sse" | jq -s '.' >"$tmp/live.json" 2>/dev/null || echo '[]' >"$tmp/live.json"

echo "== the log of the thread (the export)"
expect "the user_message holds the mention as sent" \
  "$(jq -cS '[.events[] | select(.kind == "user_message")][0].data.mentions' "$tmp/export.json")" "$(printf '%s' "$mentions" | jq -cS .)"
asks='[.events[] | select(.kind == "ask_started")]'
expect "one ask_started: the browser, ask 1, step ask-1, by main at depth 1, attributed to the chat" \
  "$(jq -r "$asks"' | map("\(.data.agent):\(.data.ask):\(.data.stepId):\(.data.by):\(.data.depth):\(.actor.name)") | join(" ")' "$tmp/export.json")" \
  "browser:1:ask-1:main:1:chat"
expect "the ask's text is the model's request: open the page the person named" \
  "$(jq -r "$asks"' | .[0].data.text // "" | startswith("Open '"$page"',")' "$tmp/export.json")" "true"
expect "its ask_finished is completed, attributed to the browser" \
  "$(jq -r '[.events[] | select(.kind == "ask_finished")] | map("\(.data.state):\(.actor.name)") | join(" ")' "$tmp/export.json")" "completed:browser"
browser_said=$(jq -r '[.events[] | select(.kind == "ask_finished")][0].data.text // ""' "$tmp/export.json")
echo "the browser said: ${browser_said:-<nothing>}"
case $browser_said in
  *"$code"*"$count"*) ok "the browser's answer holds the page's code and count (words that exist only on the page)" ;;
  *) bad "the browser's answer does not hold '$code' and '$count'" ;;
esac
case $browser_said in
  *"$page"*) ok "the browser's answer names the page's URL" ;;
  *) bad "the browser's answer does not name $page" ;;
esac

answer=$(jq -r '[.events[] | select(.kind == "agent_message" and .data.final == true) | .data.text] | last // empty' "$tmp/export.json")
[ -n "$answer" ] || answer=$(jq -r '[.events[] | select(.kind == "agent_status" and .data.state == "completed") | .data.detail] | last // empty' "$tmp/export.json")
echo "the chat said: ${answer:-<nothing>}"
case $answer in
  *"$code"*"$count"*"Source: $page"*) ok "the chat's final message names the page's code, its count and its URL" ;;
  *) bad "the chat's final message does not name '$code', '$count' and 'Source: $page'" ;;
esac

echo "== what the browser's model was sent (mock-browse)"
model_requests mock-browse >"$tmp/browse.json"
expect "five model requests: the ask, then the results of close, navigate, markdown and screenshot" "$(jq -r 'length' "$tmp/browse.json")" "5"
expect "the first ends with the ask's words" \
  "$(jq -r '.[0].messages[-1] | "\(.role):\(.content | tostring | contains("Open '"$page"',"))"' "$tmp/browse.json")" "user:true"
expect "each later one ends with the result of the call before it: browser-call-1 to browser-call-4" \
  "$(jq -r '[.[1:][] | .messages[-1] | if .role == "tool" then .tool_call_id else "none" end] | join(" ")' "$tmp/browse.json")" \
  "browser-call-1 browser-call-2 browser-call-3 browser-call-4"
expect "the task started from a clean browser: browser_close answered first" \
  "$(jq -r '.[1].messages[-1].content | tostring | test("closed"; "i")' "$tmp/browse.json")" "true"
expect "obscura opened the page: the navigate result has its title" \
  "$(jq -r '.[2].messages[-1].content | tostring | contains("Harbour Lighthouse Log")' "$tmp/browse.json")" "true"
expect "obscura read the page: the markdown result holds its code and count" \
  "$(jq -r --arg c "$code" --arg n "$count" '.[3].messages[-1].content | tostring | (contains($c) and contains($n))' "$tmp/browse.json")" "true"
if [ -n "$share_files" ]; then
  expect "the screenshot is no longer described to the model as an image not included (BROWSER_SHARE_FILES)" \
    "$(jq -r '.[4].messages[-1].content | tostring | contains("[image not included")' "$tmp/browse.json")" "false"
else
  expect "obscura took a screenshot: a PNG came back, described to the model as not included (adam-rs at the pinned revision)" \
    "$(jq -r '.[4].messages[-1].content | tostring | contains("[image not included: image/png]")' "$tmp/browse.json")" "true"
fi
offered=$(jq -r '[.[0].tools // [] | .[].function.name] | join(" ")' "$tmp/browse.json")
missing=
for t in browser__browser_close browser__browser_navigate browser__browser_markdown browser__browser_snapshot browser__browser_screenshot; do
  case " $offered " in *" $t "*) ;; *) missing="$missing $t" ;; esac
done
if [ -z "$missing" ]; then ok "the browser's model was offered the browser's tools (close, navigate, markdown, snapshot, screenshot)"; else bad "not offered:$missing (offered: $offered)"; fi
extra=
for t in browser__browser_evaluate browser__browser_set_cookie browser__browser_get_cookies browser__browser_storage_state browser__browser_set_storage_state browser__browser_fill_form ask_user show; do
  case " $offered " in *" $t "*) extra="$extra $t" ;; esac
done
if [ -z "$extra" ]; then ok "and none the folder leaves out (evaluate, cookies, storage state, the bulk form fill, ask_user, show)"; else bad "offered although left out:$extra"; fi

echo "== what the chat's model was sent (mock-persona)"
model_requests mock-persona >"$tmp/persona.json"
expect "two model requests: the ask and the answer" "$(jq -r 'length' "$tmp/persona.json")" "2"
expect "the chat's model was offered ask_agent" "$(jq -r '[.[0].tools // [] | .[].function.name] | index("ask_agent") != null' "$tmp/persona.json")" "true"
expect "its second request ends with the browser's answer (browse-ask-1, the code in it)" \
  "$(jq -r --arg c "$code" '.[1].messages[-1] | "\(.tool_call_id):\(.content | tostring | contains($c))"' "$tmp/persona.json")" "browse-ask-1:true"

echo "== the screenshot as a file (TODO: adam-rs's per-server opt-in)"
if [ -n "$share_files" ]; then
  expect "ask_finished names one image/png file" \
    "$(jq -r '[.events[] | select(.kind == "ask_finished")][0].data.artifacts // [] | map(select(.mimeType == "image/png")) | length' "$tmp/export.json")" "1"
  href=$(jq -r '[.events[] | select(.kind == "ask_finished")][0].data.artifacts // [] | map(select(.mimeType == "image/png"))[0].uri // ""' "$tmp/export.json")
  case $href in
    /api/*) expect "the API serves it to the owner as a PNG" "$(curl -sS --max-time 30 -H "$id_header" "$base$href" | head -c 4 | od -An -c | tr -d ' ')" "211PNG" ;;
    *) bad "the file's reference is not an API path: '$href'" ;;
  esac
else
  skip "BROWSER_SHARE_FILES is not set: adam-rs does not hand an MCP image over as a file at the pinned revision (the TODO in the header)"
fi

echo "== what a live viewer reads (the AG-UI stream)"
expect "SUBAGENT_STARTED sub-ask-1, named browser, under the chat's own invocation" \
  "$(jq -r '([.[] | select(.type == "SUBAGENT_STARTED" and (.subagentRunId | startswith("sub-ask-") | not))][0].subagentRunId) as $p
     | [.[] | select(.type == "SUBAGENT_STARTED" and (.subagentRunId | startswith("sub-ask-")))] | map("\(.subagentRunId):\(.name):\(.parentSubagentRunId == $p)") | join(" ")' "$tmp/live.json")" \
  "sub-ask-1:browser:true"
expect "and it ends SUBAGENT_FINISHED completed" \
  "$(jq -r '[.[] | select(.type == "SUBAGENT_FINISHED" and .subagentRunId == "sub-ask-1") | .result.state] | join(" ")' "$tmp/live.json")" "completed"

echo "== obscura's bearer"
if command -v docker >/dev/null 2>&1 && docker compose ps --status running --services 2>/dev/null | grep -qx browser; then
  # From inside the browser agent's container, which shares the sidecar's namespace: a plain request with no Authorization header.
  status_line=$(docker compose exec -T browser bash -c 'exec 3<>/dev/tcp/127.0.0.1/9223 &&
    printf "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}" >&3 &&
    head -n 1 <&3' 2>/dev/null | tr -d '\r' || true)
  expect "obscura refuses a request without its bearer" "$status_line" "HTTP/1.1 401 Unauthorized"
else
  skip "docker compose with a running browser service is not available here: the bearer check needs it"
fi

unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "mock-model matched every request" "$unmatched" "0"

finish
