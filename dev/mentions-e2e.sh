#!/usr/bin/env sh
# System-level test of mentions (ADR 0026, MVP slice 10, plan 11 PR-23), with the owner's own example (docs/vision.md, capability 4): "Help me
# understand football in Europe from 2011 till 2019. @researcher first check for data from that period and @browser you look for pictures to
# illustrate this experiment. And then @coder will plot the whole thing." The agent addressed is `chat` (adam-agent, a folder, from the coder's
# pinned image) on the scripted `mock-persona`, whose `[mock:football]` script (dev/wiremock/model/mappings/football*.json, ours) calls the thread
# tool `ask_agent` three times, one a turn and in order, then names the three answers it finds in the results. The three agents it asks are mocks:
# `mock-researcher` and `mock-browser` (WireMock A2A agents, dev/wiremock/researcher and dev/wiremock/browser, answering "Data: ..." and
# "Pictures: ...") and `mock-coder` (the WireMock `mock-agent`, which answers a message that holds `[mock:football]` with "Plot: ..."). The
# person's labels are @researcher, @browser and @coder; the references carry the ids. Nothing here talks to an agent: the script speaks AG-UI to
# the orchestrator through the edge, and reads what the mocks and the model were sent from their request journals.
#
#   dev/mentions-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; `chat` runs from the same image, with `adam-agent` and a
# folder, so there is nothing more to pull):
#
#   docker compose --profile app up -d --build --wait
#
# THE LIMIT OF WHAT IS ASSERTED. The asked agents are WireMock stand-ins, which answer with fixed words and cannot call a tool. What the log
# holds of an asked agent's work is what the orchestrator sees of it: the `ask_started` and `ask_finished` events and, for a tool call the asked
# agent makes through the thread-tools endpoint (a relayed tool of an attached server), an `agent_step` under its `ask-<n>`
# (docs/api/thread-tools-v1.md, "The child task": its own messages and steps are not copied into the thread). A mock makes no such call, so
# this script cannot assert a child step under `ask-<n>`; it asserts the ask itself as the step `ask-<n>` (`stepId`, `by`, the subagent
# `sub-ask-<n>` and its parent in the AG-UI stream) and what each asked agent was sent and answered. The child steps under an ask are asserted by the
# Rust tests (`cargo test -p orch-e2e --test ask_agent`, `-p orch-surface-thread-tools --test ask`) and are what a coder asked with an attached
# web search would show; they need a real asked agent (an adam agent) and are not part of this scenario. The script prints how many
# `agent_step` events sit under an `ask-<n>` path ("note:"), and does not count them as a check.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/agents lists `chat`, `mock-researcher`, `mock-browser` and `mock-coder`; the capabilities of `chat` list mentions/v1 and thread-tools/v1
#     (the composer can mention agents, the thread's tools reach the agent) and those of the plain `mock-coder` have no mentions key;
#   * A MENTION OF NOBODY, `forwardedProps["vymalo.mentions"]` naming the agent `nobody`: the run is 422 ("unknown agent 'nobody' in mentions"), no
#     thread is created (404) and the model was never asked: nothing is written;
#   * THE FOOTBALL REQUEST, with three mentions (ASCII text, so that the offsets are bytes, and UTF-16 code units, and characters): the run ends
#     with RUN_FINISHED (success) and the thread `done`; the `user_message` of the log holds the three references as sent, each label at its offset;
#   * the log has three `ask_started` by `main`, depth 1, in the order researcher, browser, coder (asks 1, 2, 3, steps `ask-1`..`ask-3`, attributed
#     to `chat`, its text the model's request), and each `ask_finished` is `completed`, attributed to the agent that answered, its text the agent's
#     own words; they come one after the other (started 1, finished 1, started 2, ...: each ask is answered before the next is made);
#   * what each asked agent was sent: exactly one request each, with the bearer token of dev/agents.yaml, in the context `<thread>-ask-<agentId>`, the
#     words the model asked with (the log's `ask_started` text) and none of the conversation (no label, nothing the person wrote); the coder's message
#     carries the researcher's and the browser's answers, so the results of the first two asks were used;
#   * the thread holds no message, step or artifact of the asked agents (what they say is read for the answer, never copied) and the asked agents
#     are never judged by a gate;
#   * the chat's final message names all three answers (the model took them from the three tool results: it was sent them, with the ids `fb-call-1`..`3`);
#     the model was offered `ask_agent`, and asked four times (three calls and the answer), each of the later requests ending with the result of the one before;
#   * the AG-UI stream, as the run streamed it and as the connect replay reads it: `SUBAGENT_STARTED` `sub-ask-1`..`sub-ask-3`, named after the asked
#     agents, each with the chat agent's own invocation as `parentSubagentRunId`, each `SUBAGENT_FINISHED` `completed`, and one `vymalo.ask` activity
#     per ask that ends `completed` with the answer, in the subagent of the agent that asked;
#   * `mock-model` matched every request.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL              http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL            dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL        http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   MOCK_RESEARCHER_URL   http://127.0.0.1:${MOCK_RESEARCHER_PORT:-8086}
#   MOCK_BROWSER_URL      http://127.0.0.1:${MOCK_BROWSER_PORT:-8087}
#   MOCK_CODER_URL        http://127.0.0.1:${MOCK_AGENT_PORT:-8081}, the WireMock `mock-agent` that is `mock-coder`
#   MOCK_AGENT_TOKEN      dev-mock-token   what compose.yaml sets the token of the mocks to (named by `tokenEnv` in dev/agents.yaml)
#   TIMEOUT               120    seconds to wait for a thread to stop
#
# It EMPTIES the request journals of `mock-model`, `mock-researcher`, `mock-browser` and `mock-agent` first, so run it on a stack you are not in
# the middle of another scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
researcher=${MOCK_RESEARCHER_URL:-http://127.0.0.1:${MOCK_RESEARCHER_PORT:-8086}}
researcher=${researcher%/}
browser=${MOCK_BROWSER_URL:-http://127.0.0.1:${MOCK_BROWSER_PORT:-8087}}
browser=${browser%/}
coder=${MOCK_CODER_URL:-http://127.0.0.1:${MOCK_AGENT_PORT:-8081}}
coder=${coder%/}
agent_token=${MOCK_AGENT_TOKEN:-dev-mock-token}
timeout=${TIMEOUT:-120}

mentions_uri=https://agents.vymalo.com/a2a/extensions/mentions/v1
tools_uri=https://agents.vymalo.com/a2a/extensions/thread-tools/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "mentions e2e passed"; else echo "mentions e2e FAILED"; exit 1; fi
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

# run_agui AGENT THREAD TEXT [FORWARDED_PROPS_JSON]: one run (a message) on the thread, to its end. Prints the HTTP status; the body is $tmp/run.sse.
run_agui() {
  _props=${4:-}
  [ -n "$_props" ] || _props='{}'
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" --argjson props "$_props" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

run_outcome() { # run_outcome FILE: how the run saved in FILE ended
  sse_events "$1" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
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

# requests_of URL [NAME]: the bodies of the requests a WireMock got, as a JSON array, the oldest first (WireMock's journal lists the newest
# first). With NAME, only the chat completions for that model (the model mock); without, the A2A calls (POST /a2a) with their bearer token.
requests_of() {
  if [ $# -ge 2 ]; then
    curl -s --max-time 30 "$1/__admin/requests" |
      jq -c --arg m "$2" '[.requests | reverse | .[].request.body | fromjson? | select(.model == $m)]' 2>/dev/null || echo '[]'
  else
    curl -s --max-time 30 "$1/__admin/requests" |
      jq -c '[.requests | reverse | .[] | select(.request.url == "/a2a" and .request.method == "POST")
              | {auth: (.request.headers.Authorization // .request.headers.authorization // ""), body: (.request.body | fromjson? // {})}]' 2>/dev/null || echo '[]'
  fi
}

reset_journal() { # reset_journal URL
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$1/__admin/requests" || true)
  if [ "$_code" = 200 ]; then ok "journal reset: $1"; else bad "journal reset: $1 answered HTTP $_code"; fi
}

# --- the stack, and the agents the scenario needs -------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
for a in chat mock-researcher mock-browser mock-coder; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done
ok "GET /api/agents lists chat, mock-researcher, mock-browser and mock-coder"

echo "== what an agent can do with a mention, before anyone sends"
chat_caps=$(api GET /agui/agents/chat/capabilities 2>/dev/null || echo '{}')
expect "the capabilities of chat list mentions/v1 (the composer may offer agents) and thread-tools/v1 (ask_agent can reach it)" \
  "$(printf '%s' "$chat_caps" | jq -r --arg m "$mentions_uri" --arg t "$tools_uri" '(.custom // {}) | [has($m), has($t)] | join(" ")')" "true true"
plain_caps=$(api GET /agui/agents/mock-coder/capabilities 2>/dev/null || echo 'none')
expect "the capabilities of the plain agent mock-coder were read (a document with its identity)" \
  "$(printf '%s' "$plain_caps" | jq -r '.identity.name // "none"')" "Mock coder"
expect "and have no mentions key (a card that does not list the extension gets the text as it is, and the web says so)" \
  "$(printf '%s' "$plain_caps" | jq -r '[(.custom // {}) | keys[] | select(contains("mentions"))] | length')" "0"

# --- the journals start empty ----------------------------------------------------------------------------------------
for m in "$model" "$researcher" "$browser" "$coder"; do reset_journal "$m"; done

# --- a mention of nobody ---------------------------------------------------------------------------------------------
echo "== a mention of nobody"
nobody_thread=$(uuid)
nobody_text='Hello @nobody, are you there? [mock:football]'
nobody_before=${nobody_text%%@nobody*}
nobody_start=${#nobody_before}
nobody_mentions=$(jq -n --argjson start "$nobody_start" '[{agentId: "nobody", label: "@nobody", start: $start, end: ($start + 7)}]')
code=$(run_agui chat "$nobody_thread" "$nobody_text" "$(jq -n --argjson m "$nobody_mentions" '{"vymalo.mentions": $m}')")
expect "a mention of an agent nobody knows is refused: 422" "$code" "422"
case $(cat "$tmp/run.sse") in
  *"unknown agent 'nobody' in mentions"*) ok "and says which one: unknown agent 'nobody' in mentions" ;;
  *) bad "the 422 does not say \"unknown agent 'nobody' in mentions\": $(head -c 300 "$tmp/run.sse")" ;;
esac
expect "nothing is written: no thread was created" \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -H "$id_header" "$base/api/threads/$nobody_thread")" "404"
expect "and the model was never asked" "$(requests_of "$model" mock-persona | jq -r 'length')" "0"

# --- the football request ----------------------------------------------------------------------------------------------
thread=$(uuid)
text='[mock:football] Help me understand football in Europe from 2011 till 2019. @researcher first check for data from that period and @browser you look for pictures to illustrate this experiment. And then @coder will plot the whole thing.'
# Where each label stands: the text is ASCII, so a character, a byte and a UTF-16 code unit are one. `mock-researcher` is the id the registry
# (dev/agents.yaml) gives the agent whose label the person sees as @researcher: the label is never read as an id.
mentions='[]'
for pair in "researcher:mock-researcher" "browser:mock-browser" "coder:mock-coder"; do
  label="@${pair%%:*}"
  id=${pair#*:}
  before=${text%%"$label"*}
  mentions=$(printf '%s' "$mentions" | jq -c --arg id "$id" --arg label "$label" --argjson start "${#before}" \
    '. + [{agentId: $id, label: $label, start: $start, end: ($start + ($label | length))}]')
done
echo "== the football request: thread $thread (chat), mentioning $(printf '%s' "$mentions" | jq -r '[.[].label] | join(" ")')"
code=$(run_agui chat "$thread" "$text" "$(jq -n --argjson m "$mentions" '{"vymalo.mentions": $m}')")
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/chat answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
cp "$tmp/run.sse" "$tmp/main.run.sse"
expect "the run stream ended with RUN_FINISHED (success)" "$(run_outcome "$tmp/main.run.sse")" "success"
state=$(wait_state "$thread" "done" blocked)
expect "the thread ended done (a chat answers, it does not wait)" "$state" "done"
api GET "/api/threads/$thread/export" >"$tmp/export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/export.json"
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null |
  sed -n 's/^data: *//p' | jq -s '.' >"$tmp/replay.json" 2>/dev/null || echo '[]' >"$tmp/replay.json"
sse_events "$tmp/main.run.sse" | jq -s '.' >"$tmp/live.json" 2>/dev/null || echo '[]' >"$tmp/live.json"

echo "== the log of the thread (the export)"
expect "the export is a thread export" "$(jq -r '.format' "$tmp/export.json")" "another-agentic-system/thread-export"
expect "the user_message holds the three references, as sent (ids, labels, offsets; keys in any order)" \
  "$(jq -cS '[.events[] | select(.kind == "user_message")][0].data.mentions' "$tmp/export.json")" "$(printf '%s' "$mentions" | jq -cS .)"
expect "each label stands at its offsets in the text" \
  "$(jq -r '[.events[] | select(.kind == "user_message")][0] | .data.text as $t | [.data.mentions[] | $t[.start:.end]] | join(" ")' "$tmp/export.json")" "@researcher @browser @coder"

asks='[.events[] | select(.kind == "ask_started")]'
expect "three ask_started, the researcher, then the browser, then the coder" \
  "$(jq -r "$asks"' | map(.data.agent) | join(" ")' "$tmp/export.json")" "mock-researcher mock-browser mock-coder"
expect "numbered 1, 2, 3, as the steps ask-1, ask-2, ask-3 (the ask is a step of its own)" \
  "$(jq -r "$asks"' | map("\(.data.ask):\(.data.stepId)") | join(" ")' "$tmp/export.json")" "1:ask-1 2:ask-2 3:ask-3"
expect "every one is by main, at depth 1 (the addressed agent asked)" \
  "$(jq -r "$asks"' | map("\(.data.by):\(.data.depth)") | unique | join(" ")' "$tmp/export.json")" "main:1"
expect "and attributed to the agent that asked: chat" \
  "$(jq -r "$asks"' | map(.actor.name) | unique | join(" ")' "$tmp/export.json")" "chat"
expect "each ask is answered before the next is made: started, finished, started, finished, started, finished" \
  "$(jq -r '[.events[] | select(.kind == "ask_started" or .kind == "ask_finished") | "\(.kind | sub("ask_"; ""))\(.data.ask)"] | join(" ")' "$tmp/export.json")" \
  "started1 finished1 started2 finished2 started3 finished3"
expect "each ask_finished is completed" \
  "$(jq -r '[.events[] | select(.kind == "ask_finished") | .data.state] | unique | join(" ")' "$tmp/export.json")" "completed"
expect "attributed to the agent that answered" \
  "$(jq -r '[.events[] | select(.kind == "ask_finished") | .actor.name] | join(" ")' "$tmp/export.json")" "mock-researcher mock-browser mock-coder"
expect "with the agents' own words: Data:, Pictures:, Plot:" \
  "$(jq -r '[.events[] | select(.kind == "ask_finished") | .data.text | split(" ")[0]] | join(" ")' "$tmp/export.json")" "Data: Pictures: Plot:"
expect "the researcher's ask is the model's request for data, the browser's for pictures" \
  "$(jq -r "$asks"' | [(.[0].data.text | startswith("Find data on football")), (.[1].data.text | startswith("Find pictures"))] | join(" ")' "$tmp/export.json")" "true true"
data_answer=$(jq -r '[.events[] | select(.kind == "ask_finished" and .data.ask == 1)][0].data.text // ""' "$tmp/export.json")
pics_answer=$(jq -r '[.events[] | select(.kind == "ask_finished" and .data.ask == 2)][0].data.text // ""' "$tmp/export.json")
plot_answer=$(jq -r '[.events[] | select(.kind == "ask_finished" and .data.ask == 3)][0].data.text // ""' "$tmp/export.json")
echo "the researcher said: ${data_answer:-<nothing>}"
echo "the browser said:    ${pics_answer:-<nothing>}"
echo "the coder said:      ${plot_answer:-<nothing>}"
case $(jq -r "$asks"' | .[2].data.text' "$tmp/export.json") in
  *"$data_answer"*"$pics_answer"*) ok "the coder was asked with the researcher's and the browser's answers in its request (the results of ask 1 and 2 were used)" ;;
  *) bad "the coder's request does not hold both answers, in order (data: '$data_answer', pictures: '$pics_answer')" ;;
esac
expect "the thread holds no message, step or artifact of the asked agents (their own words are read for the answer, never copied into it)" \
  "$(jq -r '[.events[] | select((.kind == "agent_message" or .kind == "agent_step" or .kind == "artifact" or .kind == "agent_status") and (.actor.name | IN("mock-researcher", "mock-browser", "mock-coder")))] | length' "$tmp/export.json")" "0"
expect "no gate judged an asked agent: no check_result of the thread names one of them" \
  "$(jq -r '[.events[] | select(.kind == "check_result" and ((.data | tostring) | test("mock-researcher|mock-browser|mock-coder")))] | length' "$tmp/export.json")" "0"
echo "note: $(jq -r '[.events[] | select(.kind == "agent_step" and ((.data.path // []) | .[0] // "" | test("^ask-[0-9]+$")))] | length' "$tmp/export.json") agent_step events sit under an ask-<n> path (the relayed tool calls of the asked agents; the WireMock agents make none, see the header)"

answer=$(jq -r '[.events[] | select(.kind == "agent_message" and .data.final == true) | .data.text] | last // empty' "$tmp/export.json")
[ -n "$answer" ] || answer=$(jq -r '[.events[] | select(.kind == "agent_status" and .data.state == "completed") | .data.detail] | last // empty' "$tmp/export.json")
echo "the chat said: ${answer:-<nothing>}"
for pair in "researcher:$data_answer" "browser:$pics_answer" "coder:$plot_answer"; do
  who=${pair%%:*}
  said=${pair#*:}
  if [ -z "$said" ]; then
    bad "the $who's answer is empty"
  else
    case $answer in
      *"$said"*) ok "the chat's final message names the $who's answer" ;;
      *) bad "the chat's final message does not name the $who's answer ('$said')" ;;
    esac
  fi
done
case $answer in
  *"(missing)"* | *"ask_agent was not offered"*) bad "the chat says an answer is missing or that ask_agent was not offered: the results did not go back to the model, or the tool did not reach it" ;;
  *) ok "the chat's final message is not the script's fallback" ;;
esac

echo "== what the asked agents were sent"
for spec in "researcher $researcher mock-researcher 1" "browser $browser mock-browser 2" "coder $coder mock-coder 3"; do
  # shellcheck disable=SC2086 # the spec is four words on purpose
  set -- $spec
  who=$1
  url=$2
  id=$3
  ask=$4
  requests_of "$url" >"$tmp/$who.requests.json"
  sends="[.[] | select(.body.method == \"SendStreamingMessage\")]"
  expect "$id got exactly one SendStreamingMessage" "$(jq -r "$sends"' | length' "$tmp/$who.requests.json")" "1"
  expect "$id: with the bearer token of dev/agents.yaml" "$(jq -r "$sends"' | .[0].auth // ""' "$tmp/$who.requests.json")" "Bearer $agent_token"
  expect "$id: in the context <thread>-ask-<agentId>, a context of its own (never the thread's)" \
    "$(jq -r "$sends"' | .[0].body.params.message.contextId // ""' "$tmp/$who.requests.json")" "$thread-ask-$id"
  expect "$id: the words it was sent are the ask's own text in the log" \
    "$(jq -r "$sends"' | .[0].body.params.message.parts[0].text // ""' "$tmp/$who.requests.json")" \
    "$(jq -r --argjson n "$ask" '[.events[] | select(.kind == "ask_started" and .data.ask == $n)][0].data.text // ""' "$tmp/export.json")"
  case $(jq -r "$sends"' | .[0].body.params.message.parts[0].text // ""' "$tmp/$who.requests.json") in
    *"@researcher"* | *"@browser"* | *"@coder"* | *"Help me understand"* | *"[mock:football] Help"*) bad "$id was sent the person's conversation (a label or the person's words are in its message)" ;;
    *) ok "$id was sent none of the conversation: the asked agent does not see it" ;;
  esac
done

echo "== what the model was sent"
persona_requests=$(requests_of "$model" mock-persona)
expect "four model requests: the three calls and the answer" "$(printf '%s' "$persona_requests" | jq -r 'length')" "4"
offered=$(printf '%s' "$persona_requests" | jq -r '[.[0].tools // [] | .[].function.name] | join(" ")')
case " $offered " in
  *" ask_agent "*) ok "the model was offered ask_agent, the tool of the thread's endpoint (tools: $offered)" ;;
  *) bad "the model was offered '${offered:-no tool}', want ask_agent among them (did the grant of the thread's tools reach chat? did the run carry mentions?)" ;;
esac
expect "each request ends with the result of the ask before it: none, fb-call-1, fb-call-2, fb-call-3" \
  "$(printf '%s' "$persona_requests" | jq -r '[.[] | (.messages[-1] | if .role == "tool" then .tool_call_id else "none" end)] | join(" ")')" "none fb-call-1 fb-call-2 fb-call-3"
expect "the last request holds the three results, one per call" \
  "$(printf '%s' "$persona_requests" | jq -r '[.[3].messages[] | select(.role == "tool") | .tool_call_id] | join(" ")')" "fb-call-1 fb-call-2 fb-call-3"
expect "and each result is the asked agent's answer" \
  "$(printf '%s' "$persona_requests" | jq -r '[.[3].messages[] | select(.role == "tool") | (.content | tostring) | (if contains("Data: ") then "data" elif contains("Pictures: ") then "pictures" elif contains("Plot: ") then "plot" else "other" end)] | join(" ")')" \
  "data pictures plot"
expect "the model was sent the person's words with the labels in them, as written (the text is not rewritten)" \
  "$(printf '%s' "$persona_requests" | jq -r '[.[0].messages[] | select(.role == "user") | .content | tostring] | last | contains("@researcher first check for data") and contains("@coder will plot")')" "true"

echo "== what a live viewer reads (the AG-UI stream)"
# check_frames LABEL FILE: the frames in FILE (a JSON array of AG-UI events) tell the football request as a chat agent with three subagents under it.
check_frames() {
  _label=$1
  _file=$2
  _started='[.[] | select(.type == "SUBAGENT_STARTED")]'
  expect "$_label: SUBAGENT_STARTED sub-ask-1, sub-ask-2, sub-ask-3, in this order, named after the asked agents" \
    "$(jq -r "$_started"' | map(select(.subagentRunId | startswith("sub-ask-"))) | map("\(.subagentRunId):\(.name)") | join(" ")' "$_file")" \
    "sub-ask-1:mock-researcher sub-ask-2:mock-browser sub-ask-3:mock-coder"
  expect "$_label: the chat agent's own invocation is the one other subagent, named chat, with no parent" \
    "$(jq -r "$_started"' | map(select(.subagentRunId | startswith("sub-ask-") | not)) | map("\(.name):\(.parentSubagentRunId // "none")") | join(" ")' "$_file")" "chat:none"
  _invocation=$(jq -r "$_started"' | map(select(.subagentRunId | startswith("sub-ask-") | not)) | .[0].subagentRunId // "none"' "$_file")
  expect "$_label: each sub-ask has the chat agent's run ($_invocation) as parentSubagentRunId" \
    "$(jq -r --arg p "$_invocation" "$_started"' | map(select(.subagentRunId | startswith("sub-ask-"))) | map(.parentSubagentRunId == $p) | unique | join(" ")' "$_file")" "true"
  expect "$_label: each sub-ask ends SUBAGENT_FINISHED completed, and none ends in SUBAGENT_ERROR" \
    "$(jq -r '([.[] | select(.type == "SUBAGENT_FINISHED" and (.subagentRunId | startswith("sub-ask-"))) | "\(.subagentRunId):\(.result.state)"] | join(" ")) as $finished
       | ([.[] | select(.type == "SUBAGENT_ERROR")] | length) as $errors | "\($finished) | \($errors)"' "$_file")" \
    "sub-ask-1:completed sub-ask-2:completed sub-ask-3:completed | 0"
  expect "$_label: one vymalo.ask activity per ask ends completed, with the agent and the answer, in the subagent of the agent that asked" \
    "$(jq -r --arg p "$_invocation" '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ask")] | group_by(.messageId) | map(last)
       | map("\(.messageId):\(.content.agent):\(.content.state):\(.content.by):\(.subagentRunId == $p):\(.content.answer | split(" ")[0])") | join(" ")' "$_file")" \
    "ask-1:mock-researcher:completed:main:true:Data: ask-2:mock-browser:completed:main:true:Pictures: ask-3:mock-coder:completed:main:true:Plot:"
}
check_frames "the run's stream" "$tmp/live.json"
check_frames "the connect replay" "$tmp/replay.json"

# --- nothing off-script -------------------------------------------------------------------------------------------------
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "mock-model matched every request" "$unmatched" "0"

finish
