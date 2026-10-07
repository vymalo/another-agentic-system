#!/usr/bin/env sh
# System-level test of the agents beside the coder (MVP slice 2): the stack lists three agents (Adam, the coder, `chat`, `researcher`), and each
# answers in its role on the mocks, through the orchestrator.
#
#   dev/agents-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; `chat` and `researcher` run
# from the same image, with `adam-agent` and a folder each, so there is nothing more to pull):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the UI does (docs/api/agui.md): one POST /agui/agents/{agentId} per thread, the
# thread's state from GET /api/threads/{id}, and the agent's words from the thread's frames
# (GET /agui/threads/{id}/connect?mode=run). The models are scripts on WireMock (dev/wiremock/model, the
# `mock-model` service): `mock-persona` greets from the first two lines of the agent's instructions, and
# `mock-researcher` calls `search__web_search` and names the first link of what comes back.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/agents lists `adam chat researcher` first, in that order, and the card of each of the two new
#     agents was read (the orchestrator gives its description);
#   * CHAT, "hi": the thread ends `done` (a chat answers, it does not wait), the run stream ends with RUN_FINISHED,
#     the words say "I'm <name>" and the one-sentence summary of dev/agents/chat/agent, and do not ask for a
#     repository (the coder's greeting does); no artifact; `mock-model` saw a `mock-persona` request whose system
#     prompt is the folder's instructions and that offered no tool of the coder or of the search, and offered `turn_output`;
#   * RESEARCHER, "Who won the football world cup in 2014?": the thread ends `done`, the words cite a link of the
#     mock web search (https://example.org/mock-search/...), `mock-mcp-search` was called exactly once, with
#     `web_search` and a query that holds the person's words, and the model got the tool `search__web_search`
#     from the folder's mcp.json (and `turn_output`) and the results back (two requests: the call, then the answer); the call is one step
#     (`vymalo.step`, kind tool) labelled with the tool's title, `Web search` (adam-rs d56dd94: not `search__web_search`), whose start
#     carries the call's `input` (the query) and whose end carries its `output` (`{text}`, the mock's list of links), as the replay says it
#     (docs/api/steps-v1.md, "Input and output");
#   * CHAT, a plan (`[mock:plan]`, ADR 0050): the chat was offered its three helpers, the sub-agents of its folder (`researcher`,
#     `writer`, `planner`); it calls `planner`, which runs as a run of its own with its own prompt; the call is one sub-agent step; the
#     plan comes back to the chat, which shows its goal and its questions and asks the person to say go;
#   * ADAM (the coder), "hi" (cheap, and the contrast): the thread ends `blocked` and the words say "I'm Adam" (its folder's name, vendored,
#     since adam-rs 4363924; the name the orchestrator lists is the agents file's, also Adam);
#   * `mock-model` matched every request.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL              http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL            dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL        http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   MOCK_MCP_SEARCH_URL   http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}
#   CHAT_AGENT_DIR        dev/agents/chat/agent             the folder the chat runs on; its name and summary are read from it
#   RESEARCHER_AGENT_DIR  dev/agents/researcher/agent       the same for the researcher
#   TIMEOUT               120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-model` and the call journal of `mock-mcp-search` first, so run it on a
# stack you are not in the middle of another scenario on.
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
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
timeout=${TIMEOUT:-120}

root=$(cd "$(dirname "$0")/.." && pwd)
chat_dir=${CHAT_AGENT_DIR:-$root/dev/agents/chat/agent}
researcher_dir=${RESEARCHER_AGENT_DIR:-$root/dev/agents/researcher/agent}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "agents e2e passed"; else echo "agents e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, one JSON per line
  sed -n 's/^data: *//p' "$1"
}

# persona DIR: sets name and summary from an agent folder: `display_name: <name>` under `vars` in the
# frontmatter, and the line `In one sentence: <summary>.` that opens the body. The mock ends the summary at its
# first period, so it holds none.
persona() {
  instructions=$1/instructions.md
  if [ ! -f "$instructions" ]; then
    echo "FAIL $instructions does not exist (the folder that holds instructions.md)"
    exit 1
  fi
  name=$(sed -n 's/^[[:space:]]*display_name:[[:space:]]*//p' "$instructions" | head -n 1)
  summary=$(sed -n 's/^In one sentence: \([^.]*\)\..*$/\1/p' "$instructions" | head -n 1)
  if [ -z "$name" ] || [ -z "$summary" ]; then
    echo "FAIL $instructions has no 'display_name:' var or no 'In one sentence: <summary>.' line"
    exit 1
  fi
}

# say AGENT TEXT: one thread for the agent whose first message is TEXT. Waits for the thread to stop and sets
#   state     the state the thread ended in
#   outcome   how the run stream ended (success, interrupt, error: <code>, or empty)
#   said      the words of every assistant message of the thread, joined
#   events    the file holding the thread's frames (a JSON array)
say() {
  _agent=$1
  _thread=$(uuid)
  _input=$(jq -n --arg thread "$_thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  echo "thread $_thread ($_agent): $2"
  _deadline=$(( $(date +%s) + timeout ))
  _code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/$_agent" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true)
  if [ "$_code" != 200 ]; then
    bad "POST /agui/agents/$_agent answered HTTP ${_code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
    finish
  fi
  outcome=$(sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true)
  state=
  while :; do
    state=$(api GET "/api/threads/$_thread" 2>/dev/null | jq -r '.state // empty' || true)
    case $state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 2
  done
  events=$tmp/events-$_agent.json
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
    "$base/agui/threads/$_thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
    echo '[]' > "$events"
  # The words of every assistant message of the thread, in order.
  said=$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "assistant") | .messageId] as $ids
    | [.[] | select(.type == "TEXT_MESSAGE_CONTENT" and (.messageId | IN($ids[]))) | .delta] | join(" ")' "$events" 2>/dev/null || true)
  echo "the $_agent said: ${said:-<nothing>}"
}

# why EVENTS: what the thread said about a failure, to help whoever reads the log.
why() {
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$1" | head -n 20
}

# requests MOCK-MODEL-NAME: the bodies of the requests the model mock got for that model, as a JSON array, the
# oldest first (WireMock's journal lists the newest first).
requests() {
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c --arg m "$1" '[.requests | reverse | .[].request.body | fromjson? | select(.model == $m)]' 2>/dev/null || echo '[]'
}

# --- the agents ------------------------------------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  ids=$(printf '%s' "$agents" | jq -r '[.[].id] | .[0:3] | join(" ")')
  if [ "$ids" = "adam chat researcher" ]; then
    ok "GET /api/agents starts with adam chat researcher (Adam, the coder, is the default)"
  else
    bad "GET /api/agents starts with '$ids', want 'adam chat researcher' (all: $(printf '%s' "$agents" | jq -c '[.[].id]'))"
    finish
  fi
  for a in chat researcher; do
    description=$(printf '%s' "$agents" | jq -r --arg a "$a" '.[] | select(.id == $a) | .description // empty')
    if [ -n "$description" ]; then
      ok "$a's card was read: \"$description\""
    else
      bad "$a has no description in GET /api/agents: its card could not be read (is the $a service healthy? docker compose --profile app logs $a)"
    fi
  done
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi

# --- the journals start empty --------------------------------------------------------------------------
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $code"; fi
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$search/__journal" || true)
if [ "$code" = 200 ]; then ok "journal reset: $search"; else bad "journal reset: $search answered HTTP $code"; fi

# --- the chat ---------------------------------------------------------------------------------------
persona "$chat_dir"
echo "persona of $chat_dir: $name, \"$summary\""
chat_name=$name
chat_summary=$summary
say chat "hi"
if [ "$outcome" = success ]; then
  ok "chat: the run stream ended with RUN_FINISHED (success)"
else
  bad "chat: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"
fi
if [ "$state" = "done" ]; then
  ok "chat: the thread ended done (a chat answers, it does not wait)"
else
  bad "chat: the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"
  why "$events"
fi
case $said in
  *"I'm $chat_name"*) ok "chat: it says its name: I'm $chat_name" ;;
  *) bad "chat: the answer does not say \"I'm $chat_name\"" ;;
esac
case $said in
  *"$chat_summary"*) ok "chat: it says what it does: $chat_summary" ;;
  *) bad "chat: the answer does not say what it does (\"$chat_summary\")" ;;
esac
case $said in
  *[Rr]epositor*) bad "chat: the answer talks about a repository (that is the coder's greeting, not a chat's)" ;;
  *) ok "chat: no repository talk" ;;
esac
artifacts=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact") | .content.name] | join(" ")' "$events" 2>/dev/null || true)
if [ -z "$artifacts" ]; then ok "chat: no tool ran: no artifact"; else bad "chat: a greeting produced artifacts: $artifacts"; fi
chat_requests=$(requests mock-persona)
first=$(printf '%s' "$chat_requests" | jq -r '.[0].messages[0].content // empty')
case $first in
  *"Your name is $chat_name."*"In one sentence: $chat_summary"*) ok "chat: the model's system prompt is the folder's instructions (its two persona lines)" ;;
  *) bad "chat: mock-model saw no mock-persona request whose system prompt holds \"Your name is $chat_name.\" and the one-sentence line (is the folder mounted at /etc/adam/agent the one in CHAT_AGENT_DIR?)" ;;
esac
# A chat has no tool of the coder and no search of its own: what a model is offered is what the folder and the binary give.
offered=$(printf '%s' "$chat_requests" | jq -r '[.[0].tools // [] | .[].function.name] | join(" ")')
case " $offered " in
  *" prepare_workspace "* | *" run_checks "* | *" commit_and_push "* | *" open_pull_request "* | *" search__web_search "*)
    bad "chat: the model was offered a tool the chat must not have: $offered" ;;
  *) ok "chat: the model was offered no code tool and no search (tools: ${offered:-none})" ;;
esac
# ...but it has three helpers, the sub-agents of its folder (agent/subagents/, ADR 0050), each one a tool of the chat's own.
for helper in researcher writer planner; do
  case " $offered " in
    *" $helper "*) ok "chat: the model was offered its helper $helper" ;;
    *) bad "chat: the model was not offered its helper $helper (tools: ${offered:-none}; is the folder's subagents/ mounted and valid?)" ;;
  esac
done
# The thread's own tool: the orchestrator lists `turn_output` in the grant of every message and adam-agent offers it to the model, whose
# instructions say to call it with the complete answer (adam-rs c0f12dd). The mock model does not call it: its reply is the answer.
case " $offered " in
  *" turn_output "*) ok "chat: the model was offered turn_output, the thread's tool for the answer" ;;
  *) bad "chat: the model was not offered turn_output (tools: ${offered:-none}; did the grant of the thread's tools reach the chat?)" ;;
esac

# --- the researcher -----------------------------------------------------------------------------------
persona "$researcher_dir"
researcher_name=$name
say researcher "Who won the football world cup in 2014?"
if [ "$outcome" = success ]; then
  ok "researcher: the run stream ended with RUN_FINISHED (success)"
else
  bad "researcher: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"
fi
if [ "$state" = "done" ]; then
  ok "researcher: the thread ended done"
else
  bad "researcher: the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"
  why "$events"
fi
case $said in
  *"https://example.org/mock-search/"*) ok "researcher: the answer cites a source of the mock web search" ;;
  *) bad "researcher: the answer cites no link of https://example.org/mock-search/" ;;
esac
calls=$(curl -s --max-time 30 "$search/__journal" || true)
count=$(printf '%s' "$calls" | jq -r '.calls | length' 2>/dev/null || echo '?')
if [ "$count" = 1 ]; then
  ok "researcher: mock-mcp-search got exactly one call"
else
  bad "researcher: mock-mcp-search got $count calls, want exactly one ($(printf '%s' "$calls" | jq -c '.calls' 2>/dev/null || echo "$calls"))"
fi
tool=$(printf '%s' "$calls" | jq -r '.calls[0].tool // empty' 2>/dev/null || true)
query=$(printf '%s' "$calls" | jq -r '.calls[0].arguments.query // empty' 2>/dev/null || true)
if [ "$tool" = web_search ] && [ -n "$query" ]; then
  ok "researcher: the call was web_search with the query \"$query\""
else
  bad "researcher: the call was '${tool:-none}' with the query '${query:-}', want web_search and a non-empty query"
fi
case $query in
  *football*) ok "researcher: the query holds the person's words" ;;
  *) bad "researcher: the query \"$query\" does not hold the person's words (football)" ;;
esac
researcher_requests=$(requests mock-researcher)
turns=$(printf '%s' "$researcher_requests" | jq -r 'length')
if [ "$turns" = 2 ]; then
  ok "researcher: two model requests, the call and then the answer"
else
  bad "researcher: $turns model requests, want 2 (the tool call, then the answer)"
fi
offered=$(printf '%s' "$researcher_requests" | jq -r '[.[0].tools // [] | .[].function.name] | join(" ")')
case " $offered " in
  *" search__web_search "*) ok "researcher: the model was offered search__web_search, the tool of its mcp.json (tools: $offered)" ;;
  *) bad "researcher: the model was offered '${offered:-no tool}', want search__web_search (is mock-mcp-search up, and MCP_ALLOW_INSECURE set?)" ;;
esac
case " $offered " in
  *" turn_output "*) ok "researcher: the model was offered turn_output, the thread's tool for the answer" ;;
  *) bad "researcher: the model was not offered turn_output (tools: ${offered:-none})" ;;
esac
first=$(printf '%s' "$researcher_requests" | jq -r '.[0].messages[0].content // empty')
case $first in
  *"Your name is $researcher_name."*) ok "researcher: the model's system prompt is the folder's instructions" ;;
  *) bad "researcher: the system prompt does not hold \"Your name is $researcher_name.\" (is the folder mounted at /etc/adam/agent the one in RESEARCHER_AGENT_DIR?)" ;;
esac
result=$(printf '%s' "$researcher_requests" | jq -r '[.[1].messages // [] | .[] | select(.role == "tool") | .content] | last // empty')
case $result in
  *"https://example.org/mock-search/"*) ok "researcher: the results of the search went back to the model" ;;
  *) bad "researcher: the second model request holds no search result (tool message: '${result:-none}')" ;;
esac

# The call as a step (steps/v1, adam-rs d56dd94: a tool step carries its input and its output, and an MCP tool's `title` is its label). The
# replay holds one `vymalo.step` snapshot per report of the step, the start first and the end last, and the end says the step as it stands.
# The mock search server gives `Web search` as the title of its one tool (dev/mock-mcp-search/server.mjs), so that is the label; without a
# title it would be `search__web_search`, as the model knows the tool.
# shellcheck disable=SC2016 # jq's own variables, not the shell's
steps_def='def steps: .[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step"); '
search_steps=$(jq -c "$steps_def"'[steps | select(.content.kind == "tool" and .content.label == "Web search")]' "$events" 2>/dev/null || echo '[]')
n_search=$(printf '%s' "$search_steps" | jq -r 'map(.content.id) | unique | length' 2>/dev/null || echo 0)
if [ "$n_search" = 1 ]; then
  ok "researcher: the call is one step, labelled with the tool's title: Web search"
else
  seen=$(jq -r "$steps_def"'[steps | select(.content.kind == "tool") | .content.label] | unique | join(" ")' "$events" 2>/dev/null || true)
  bad "researcher: $n_search tool steps labelled 'Web search', want exactly one (tool steps seen: ${seen:-none}; is the label the MCP tool's title?)"
fi
step_state=$(printf '%s' "$search_steps" | jq -r 'last | .content.state // empty' 2>/dev/null || true)
if [ "$step_state" = completed ]; then
  ok "researcher: the step ended completed"
else
  bad "researcher: the Web search step ended '${step_state:-none}', want completed"
fi
step_query=$(printf '%s' "$search_steps" | jq -r 'first | .content.input.query // empty' 2>/dev/null || true)
case $step_query in
  *football*) ok "researcher: the step's start carries the call's input, the query \"$step_query\"" ;;
  *) bad "researcher: the step's start carries no input with the person's words (input.query: '${step_query:-none}'; steps/v1 input, adam-rs d56dd94)" ;;
esac
step_output=$(printf '%s' "$search_steps" | jq -r 'last | .content.output.text // empty' 2>/dev/null || true)
step_error=$(printf '%s' "$search_steps" | jq -r 'last | .content.output.error // false' 2>/dev/null || true)
case $step_output in
  *"https://example.org/mock-search/"*)
    if [ "$step_error" = false ]; then
      ok "researcher: the step's end carries the output, the links of the mock web search (not an error)"
    else
      bad "researcher: the step's output is marked as an error: $step_output"
    fi ;;
  *) bad "researcher: the step's end carries no output with a link of the mock web search (output.text: '${step_output:-none}'; steps/v1 output, adam-rs d56dd94)" ;;
esac

# --- the chat's helpers: a plan (ADR 0050) -----------------------------------------------------------------------------
# A sub-agent is a tool of the chat: the script `[mock:plan]` of `mock-persona` calls `planner` with a message that holds the marker of the
# helper's own run, the helper's run is answered with a one-line plan, and the chat shows its goal and its questions and waits for the person.
say chat "Help me organise a team offsite. [mock:plan]"
if [ "$outcome" = success ]; then
  ok "chat, planner: the run stream ended with RUN_FINISHED (success)"
else
  bad "chat, planner: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"
fi
if [ "$state" = "done" ]; then
  ok "chat, planner: the thread ended done"
else
  bad "chat, planner: the thread ended '${state:-unknown}', want done"
  why "$events"
fi
case $said in
  *"Goal: Organise a team offsite for twelve people"*"Questions: Which dates work and what is the budget?"*"say go"*)
    ok "chat, planner: the answer shows the helper's goal and questions and waits for the person" ;;
  *) bad "chat, planner: the answer does not show the planner's goal and questions and ask the person to say go" ;;
esac
planner_steps=$(jq -r "$steps_def"'[steps | select(.content.kind == "subagent" and ((.content.label // "") + " " + (.content.id // "") | test("planner"; "i"))) | .content.id] | unique | length' "$events" 2>/dev/null || echo 0)
if [ "${planner_steps:-0}" = 1 ]; then
  ok "chat, planner: the call of the helper is one sub-agent step"
else
  seen=$(jq -r "$steps_def"'[steps | "\(.content.kind):\(.content.label)"] | unique | join(" | ")' "$events" 2>/dev/null || true)
  bad "chat, planner: ${planner_steps:-0} sub-agent steps name the planner, want one (steps seen: ${seen:-none})"
fi
plan_requests=$(requests mock-persona)
plan_child=$(printf '%s' "$plan_requests" | jq -r '[.[] | select(.messages[0].content // "" | startswith("You are the planner")) | .messages[-1].content] | first // empty')
case $plan_child in
  "[mock:plan-sub] "*) ok "chat, planner: the helper ran as a run of its own, with its own prompt and the chat's message" ;;
  *) bad "chat, planner: no model request had the planner's system prompt and the chat's message last (seen: '${plan_child:-none}')" ;;
esac
plan_final=$(printf '%s' "$plan_requests" | jq -r '[.[] | select(.messages[0].content // "" | startswith("Your name is")) | select(.messages[-1].role == "tool") | .messages[-1].content] | last // empty')
case $plan_final in
  "Goal: "*) ok "chat, planner: the helper's result went back to the chat's model" ;;
  *) bad "chat, planner: the chat's last request holds no tool result with the plan (tool message: '${plan_final:-none}')" ;;
esac

# --- Adam, the coder, for contrast ---------------------------------------------------------------------
say adam "hi"
if [ "$state" = blocked ]; then
  ok "adam: the thread ended blocked (it asks what it can help with, where the chat answered)"
else
  bad "adam: the thread ended '${state:-unknown}', want blocked"
  why "$events"
fi
case $said in
  # The name the agent SAYS is its folder's (dev/coder/agent, vendored): "Adam" since adam-rs's own rename (adam-rs ADR 0021, 4363924) is in
  # the pin; the name the orchestrator LISTS is the agents file's, also Adam.
  *"I'm Adam"*) ok "adam: it says its name, the one of its folder: I'm Adam" ;;
  *) bad "adam: the answer does not say \"I'm Adam\" (the name in the vendored folder)" ;;
esac

# --- nothing off-script ------------------------------------------------------------------------------
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then
  ok "mock-model matched every request"
else
  bad "mock-model saw $unmatched unmatched requests (see $model/__admin/requests/unmatched)"
fi

finish
