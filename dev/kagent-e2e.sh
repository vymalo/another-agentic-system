#!/usr/bin/env sh
# System-level test of the orchestrator against a kagent agent, over plain A2A 1.0 (dev/README.md, "kagent"): kagent is an agent host like any
# other, added by its card URL, and this scenario proves what works, records what does not, and says what each costs. Two kagent lines, one script:
#
#   KAGENT_VERSION=1.x   (the default) kagent 1.0.0-alpha8, which runs every agent on Agent Substrate: a heavy cluster, weekly and on demand
#       dev/kagent/up.sh                                    # kind + Agent Substrate + kagent 1.0.0-alpha8 + one agent, `kagent/hello` (minutes)
#       docker compose -f compose.yaml -f dev/compose.kagent.yaml --profile app up -d --build --wait postgres mock-oidc orchestrator
#       dev/kagent-e2e.sh
#   KAGENT_VERSION=0.10  kagent v0.10.3, which runs an agent as a plain Deployment: a light cluster, on every pull request that touches the A2A client
#       dev/kagent/up-0.10.sh                               # kind + kagent 0.10.3 + one agent, `kagent/hello` (a few minutes)
#       KAGENT_AGENTS_FILE=./dev/kagent/agents-0.10.yaml docker compose -f compose.yaml -f dev/compose.kagent.yaml --profile app up -d --build --wait postgres mock-oidc orchestrator
#       KAGENT_VERSION=0.10 dev/kagent-e2e.sh
#
# Only the model is scripted: a WireMock in the cluster (dev/kagent/wiremock/mappings) that answers "kagent says hello" to a first turn and "kagent says
# hello again" to a turn whose history holds the first answer, and (1.x only) for `[mock:ask]` calls the runtime's `ask_user` tool (human in the loop).
#
# It prints one ok, FAIL or `finding:` line per check and exits 1 if a check failed. A `finding:` records what the other side did, which is the point of
# the scenario, and fails nothing by itself:
#   * KAGENT, directly (no orchestrator; what the other side does):
#       - `finding: auth`: the card, and a call, with no bearer, and with a made-up one: kagent's default mode reads no credential (read in its source),
#         so 200 is expected; a 401 or 403 is recorded just the same, and the orchestrator then needs a real token;
#       - the card is an A2A 1.0 card, JSON-RPC first, its URL ending in the agent's endpoint (/agents/kagent/hello in 1.x, /api/a2a/kagent/hello in 0.10);
#       - a SendMessage with NO contextId is answered by the scripted model, completed, and kagent assigns the contextId;
#       - `finding: contextId`: a SendMessage with a contextId of the caller's own making (what the orchestrator used to send: its thread id) is accepted or
#         refused; continuing the context kagent assigned works, and its history reaches the model (the control experiment);
#       - 1.x only, `finding: hitl`: a request that makes the runtime call `ask_user` pauses the task as `input-required` with the question as the status
#         message's text, with no extension activated; what its metadata holds, and what a plain-text answer on the paused task does, are recorded;
#   * THE ORCHESTRATOR, over AG-UI, as the web speaks it:
#       - GET /api/agents lists `kagent`, and its card was read (a description);
#       - MESSAGE 1, which names NO contextId (ADR 0055: kagent refuses one it did not assign): the run stream starts (RUN_STARTED) and ends RUN_FINISHED
#         (success), the thread ends `done`, the agent's message is the mock's "kagent says hello", and the thread's binding adopted kagent's context (the
#         export's `binding.contextId` is kagent's, not the thread id);
#       - MESSAGE 2 in the same thread, sent in that context: `done`, the answer is the second turn's ("again": the model's second request carries the first
#         exchange in its history), and, in 1.x, kagent holds ONE context with TWO tasks for the thread (ListTasks, read directly; recorded as a finding in 0.10);
#       - the model mock matched every request, and every request was for `kagent-mock`;
#       - 1.x only, HUMAN IN THE LOOP: a request that makes the runtime call `ask_user` pauses the A2A task as `input-required`. The orchestrator does not
#         activate kagent's extension (https://kagent.dev/extensions/hitl/v1: it knows no such URI), so what it sees is what kagent says a client that did not
#         activate it sees: a status message whose TEXT is the question. Asserted: the thread ends `blocked`, the run ends as an interrupt and the question is
#         the agent's message. Then the person answers in words: the thread's next state is RECORDED (`finding: hitl`), and only has to be a state the thread
#         ends in, not a hang;
#       - `note` lines say what the thread does not have, because the card lists none of the orchestrator's extensions: steps, streamed text, UI surfaces,
#         release channels, thread tools.
# Exit status 0 when every check passed.
#
# Environment (defaults match dev/compose.kagent.yaml and the kind config of the version on one machine):
#   KAGENT_VERSION   1.x    1.x or 0.10: which kagent, and so the ports, the endpoint path and the checks (above)
#   BASE_URL         http://127.0.0.1:${KAGENT_ORCH_PORT:-8097}   the orchestrator, published by dev/compose.kagent.yaml (no edge)
#   KAGENT_URL       http://127.0.0.1:18083 (1.x), :18093 (0.10)  kagent's controller, the A2A gateway, on the kind node's NodePort
#   MOCK_MODEL_URL   http://127.0.0.1:18080 (1.x), :18090 (0.10)  the scripted model in the cluster
#   AUTH_EMAIL       dev@example.com                              the user: the token of dev/auth-header.sh (OIDC_URL, as there) is theirs
#   KAGENT_NS, KAGENT_AGENT   kagent, hello                       the Agent the orchestrator's agents file names
#   TIMEOUT          240    seconds to wait for a thread to stop (1.x: the first message wakes an Actor)
#
# It EMPTIES the request journal of the model mock after its direct probes, so run it on a cluster you are not in the middle of something on.
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/kagent-e2e.yml.
set -eu

variant=${KAGENT_VERSION:-1.x}
case $variant in
  1.x)
    a2a_path=/agents
    default_kagent=http://127.0.0.1:18083
    default_model=http://127.0.0.1:18080
    up_script=dev/kagent/up.sh
    agents_file=
    ;;
  0.10)
    a2a_path=/api/a2a
    default_kagent=http://127.0.0.1:18093
    default_model=http://127.0.0.1:18090
    up_script=dev/kagent/up-0.10.sh
    agents_file="KAGENT_AGENTS_FILE=./dev/kagent/agents-0.10.yaml "
    ;;
  *) echo "KAGENT_VERSION is 1.x or 0.10, not '$variant'" >&2; exit 2 ;;
esac
base=${BASE_URL:-http://127.0.0.1:${KAGENT_ORCH_PORT:-8097}}
base=${base%/}
kagent=${KAGENT_URL:-$default_kagent}
kagent=${kagent%/}
model=${MOCK_MODEL_URL:-$default_model}
model=${model%/}
ns=${KAGENT_NS:-kagent}
agent=${KAGENT_AGENT:-hello}
email=${AUTH_EMAIL:-dev@example.com}
timeout=${TIMEOUT:-240}
# The API wants a bearer token of the mock issuer (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")

hitl_uri=https://kagent.dev/extensions/hitl/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finding() { echo "finding: $1"; }
note() { echo "note $1"; }
finish() {
  if [ "$fail" -ne 0 ]; then echo "kagent $variant e2e FAILED"; exit 1; fi
  echo "kagent $variant e2e passed"
  exit 0
}
expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the orchestrator's resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, as a JSON array
  sed -n 's/^data: *//p' "$1" | jq -s '.' 2>/dev/null || echo '[]'
}

# --- what is up ------------------------------------------------------------------------------------------
ready=
for _ in $(seq 1 60); do
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$base/readyz" 2>/dev/null || true)
  if [ "$code" = 200 ]; then ready=1; break; fi
  sleep 2
done
if [ -z "$ready" ]; then
  echo "the orchestrator does not answer at $base/readyz: start it with" >&2
  echo "  ${agents_file}docker compose -f compose.yaml -f dev/compose.kagent.yaml --profile app up -d --build --wait postgres mock-oidc orchestrator" >&2
  exit 2
fi
card_url=$kagent$a2a_path/$ns/$agent/.well-known/agent-card.json
if ! curl -fsS --max-time 10 -o "$tmp/card.json" "$card_url" 2>"$tmp/err"; then
  echo "kagent does not serve the card at $card_url: $(head -c 300 "$tmp/err"); is the cluster up ($up_script)?" >&2
  exit 2
fi
if ! curl -fsS --max-time 10 "$model/__admin/health" >/dev/null 2>&1; then
  echo "the model mock does not answer at $model/__admin/health ($up_script makes it)" >&2
  exit 2
fi

# ===================================================================================================
echo "== kagent, directly: what the other side does"

# rpc METHOD PARAMS [BEARER]: one JSON-RPC call to the agent's endpoint, the body in $tmp/rpc.json, the HTTP status on stdout.
rpc() {
  _auth=
  [ -z "${3:-}" ] || _auth="Authorization: Bearer $3"
  jq -n --arg m "$1" --argjson p "$2" '{jsonrpc: "2.0", id: "kagent-e2e", method: $m, params: $p}' |
    curl -sS --max-time 120 -o "$tmp/rpc.json" -w '%{http_code}' -X POST "$kagent$a2a_path/$ns/$agent" \
      -H 'content-type: application/json' -H 'A2A-Version: 1.0' ${_auth:+-H "$_auth"} --data-binary @- 2>"$tmp/rpc.err" || true
}

# message TEXT [CONTEXT_ID [TASK_ID]]: the params of a SendMessage.
message() {
  jq -n --arg id "$(uuid)" --arg text "$1" --arg ctx "${2:-}" --arg task "${3:-}" \
    '{message: ({messageId: $id, role: "ROLE_USER", parts: [{text: $text}]}
      + (if $ctx == "" then {} else {contextId: $ctx} end) + (if $task == "" then {} else {taskId: $task} end))}'
}

# task_of FILE: the task of a SendMessage answer (A2A 1.0 wraps it as {task}; read both spellings).
task_of() { jq -c '(.result.task // .result) // {}' "$1" 2>/dev/null || echo '{}'; }
state_of() { printf '%s' "$1" | jq -r '(.status.state // "") | tostring | ascii_downcase' 2>/dev/null || true; }
text_of() { printf '%s' "$1" | jq -r '[.. | objects | .text? | strings] | join(" ")' 2>/dev/null || true; }

# --- the card, and the credential ------------------------------------------------------------------------
card_none=$(curl -s -o "$tmp/card-none.json" -w '%{http_code}' --max-time 10 "$card_url" || true)
card_bogus=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 -H 'Authorization: Bearer not-a-credential' "$card_url" || true)
finding "auth: GET the card with no bearer answers HTTP $card_none, with a made-up bearer HTTP $card_bogus"
case $card_none in
  200 | 401 | 403) ok "the card answers 200, 401 or 403 with no bearer (HTTP $card_none), nothing else" ;;
  *) bad "the card answers HTTP $card_none with no bearer: neither the open answer (200) nor a refusal (401, 403)" ;;
esac

expect "the card's first interface is JSON-RPC (A2A 1.0: supportedInterfaces)" \
  "$(jq -r '.supportedInterfaces[0].protocolBinding // "none"' "$tmp/card.json")" "JSONRPC"
case $(jq -r '.supportedInterfaces[0].url // ""' "$tmp/card.json") in
  *"$a2a_path"/"$ns"/"$agent") ok "its URL is the agent's endpoint, ...$a2a_path/$ns/$agent" ;;
  *) bad "its URL is '$(jq -r '.supportedInterfaces[0].url // ""' "$tmp/card.json")', want ...$a2a_path/$ns/$agent" ;;
esac
echo "the card: name '$(jq -r '.name // ""' "$tmp/card.json")', versions $(jq -c '[.supportedInterfaces[].protocolVersion]' "$tmp/card.json"), extensions $(jq -c '[.capabilities.extensions[]?.uri]' "$tmp/card.json")"

# --- a conversation kagent starts itself -------------------------------------------------------------------
code=$(rpc SendMessage "$(message 'Say hello (direct probe)')")
direct=$(task_of "$tmp/rpc.json")
finding "auth: a SendMessage with no bearer answers HTTP $code"
if [ "$code" = 401 ] || [ "$code" = 403 ]; then
  finding "auth: kagent refuses a call with no credential: the orchestrator's agent token (tokenEnv) must be one kagent accepts, and the rest of this scenario cannot run with the dummy of compose"
  bad "kagent answered HTTP $code to a call with no bearer: the scenario's orchestrator sends a dummy token and cannot get through"
  finish
fi
ctx1=$(printf '%s' "$direct" | jq -r '.contextId // empty')
completed() { case $1 in *completed*) echo completed ;; *) echo "$1" ;; esac; } # A2A 1.0 writes TASK_STATE_COMPLETED; a lower-case spelling would do as well
expect "SendMessage with no contextId: completed" "$(completed "$(state_of "$direct")")" "completed"
case $(text_of "$direct") in
  *"kagent says hello"*) ok "the scripted model's answer is the task's text (the agent's runtime, the gateway and the model all work)" ;;
  *) bad "the task's text has no 'kagent says hello': $(head -c 300 "$tmp/rpc.json")"; finish ;;
esac
if [ -n "$ctx1" ]; then ok "kagent assigned a contextId: $ctx1"; else bad "the task has no contextId: $(head -c 300 "$tmp/rpc.json")"; finish; fi

# --- a contextId the caller made up (the orchestrator's thread id) ------------------------------------------
mine=$(uuid)
code=$(rpc SendMessage "$(message 'Say hello (own context)' "$mine")")
err=$(jq -r 'if .error then "error \(.error.code): \(.error.message)" else empty end' "$tmp/rpc.json" 2>/dev/null || true)
theirs=$(task_of "$tmp/rpc.json" | jq -r '.contextId // empty')
if [ -n "$err" ]; then
  finding "contextId: kagent REFUSES a contextId it did not assign (HTTP $code, $err): a client that makes its own context ids, as the orchestrator used to (its thread id), cannot start a conversation; since ADR 0055 the orchestrator's first message names none"
elif [ -n "$theirs" ]; then
  finding "contextId: kagent ACCEPTS a contextId the caller made up and answers in context $theirs (the caller's own: $mine)"
else
  finding "contextId: kagent answered HTTP $code with neither a task nor an error to a made-up contextId: $(head -c 300 "$tmp/rpc.json")"
fi

# --- the control experiment: kagent continues the context it assigned -------------------------------------------
code=$(rpc SendMessage "$(message 'And once more (direct probe)' "$ctx1")")
direct2=$(task_of "$tmp/rpc.json")
expect "SendMessage in the context kagent assigned: completed" "$(completed "$(state_of "$direct2")")" "completed"
case $(text_of "$direct2") in
  *"kagent says hello again"*) ok "the second turn's history reached the model: the answer is the 'again' script" ;;
  *) bad "the second direct turn was not answered by the 'again' script: $(head -c 300 "$tmp/rpc.json")" ;;
esac
expect "the same contextId" "$(printf '%s' "$direct2" | jq -r '.contextId // empty')" "$ctx1"

# --- a paused task, as a client that did not activate kagent's HITL extension sees it ---------------------------
if [ "$variant" = 1.x ]; then
  # No A2A-Extensions header, like the orchestrator's calls. The pause is `input-required` with a status message; the docs say its TEXT is the
  # question whether or not the extension is active, and its metadata (the structured request) belongs to the extension.
  code=$(rpc SendMessage "$(message "[mock:ask] Which database should we use for storage? (direct probe)")")
  paused=$(task_of "$tmp/rpc.json")
  case $(state_of "$paused") in
    *input*required*) ok "a request that makes the runtime call ask_user pauses the task as input-required" ;;
    *) bad "the ask_user request ended '$(state_of "$paused")' (HTTP $code), want input-required: $(head -c 300 "$tmp/rpc.json")" ;;
  esac
  case $(printf '%s' "$paused" | jq -r '[.status.message // empty | .. | objects | .text? | strings] | join(" ")') in
    *"Which database should we use?"*) ok "the status message's text is the question, with no extension activated" ;;
    *) bad "the status message's text is not the question: $(printf '%s' "$paused" | jq -c '.status' | head -c 300)" ;;
  esac
  finding "hitl: the pause's status message metadata, with no extension activated: $(printf '%s' "$paused" | jq -c '[.status.message.metadata // {} | keys[]]')"
  ptask=$(printf '%s' "$paused" | jq -r '.id // empty')
  pctx=$(printf '%s' "$paused" | jq -r '.contextId // empty')
  if [ -n "$ptask" ] && [ -n "$pctx" ]; then
    # What the orchestrator does when the person answers: words, on the same task and context.
    code=$(rpc SendMessage "$(message 'PostgreSQL (direct probe)' "$pctx" "$ptask")")
    resumed=$(task_of "$tmp/rpc.json")
    err=$(jq -r 'if .error then "error \(.error.code): \(.error.message)" else empty end' "$tmp/rpc.json" 2>/dev/null || true)
    after=${err:-"task state $(state_of "$resumed")"}
    finding "hitl: a plain-text answer on the paused task answers HTTP $code, $after"
  fi
fi

curl -s -o /dev/null --max-time 30 -X DELETE "$model/__admin/requests" || true

# ===================================================================================================
echo "== the orchestrator lists the agent"
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  ids=$(printf '%s' "$agents" | jq -r '[.[].id] | join(" ")')
  case " $ids " in
    *" kagent "*) ok "GET /api/agents lists kagent (all: $ids)" ;;
    *) bad "GET /api/agents does not list kagent (it lists: $ids)"; finish ;;
  esac
  description=$(printf '%s' "$agents" | jq -r '.[] | select(.id == "kagent") | .description // empty')
  if [ -n "$description" ]; then
    ok "the card was read by the orchestrator: \"$description\""
  else
    bad "kagent has no description in GET /api/agents: its card could not be read by the orchestrator (is kagent-control-plane:30083 reachable from its container: docker network kind?)"
  fi
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi

# run_message THREAD TEXT OUT: one AG-UI run (a message) on the thread; the SSE response is saved in OUT. Prints the HTTP status.
run_message() {
  _input=$(jq -n --arg thread "$1" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$3" -w '%{http_code}' -X POST "$base/agui/agents/kagent" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$3.err" || true
}

outcome_of() { # outcome_of EVENTS_JSON_FILE: how the run ended (success, interrupt, error: <code>, or empty)
  jq -r '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' "$1" 2>/dev/null || true
}

wait_state() { # wait_state THREAD: waits until the thread has ended a job, and prints its state
  _deadline=$(( $(date +%s) + timeout ))
  while :; do
    _state=$(api GET "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' || true)
    case $_state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 2
  done
  echo "${_state:-unknown}"
}

replay() { # replay THREAD OUT: the thread's frames from the log (a JSON array)
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
    "$base/agui/threads/$1/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$2" 2>/dev/null || echo '[]' > "$2"
}

said_of() { # said_of EVENTS_JSON_FILE: the words of every assistant message, joined
  jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "assistant") | .messageId] as $ids
    | [.[] | select(.type == "TEXT_MESSAGE_CONTENT" and (.messageId | IN($ids[]))) | .delta] | join(" ")' "$1" 2>/dev/null || true
}

why() { # why THREAD EVENTS: what the thread said about a failure, to help whoever reads the log
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$2" | head -n 20
  api GET "/api/threads/$1/export" 2>/dev/null |
    jq -r '.events[]? | select(.kind | test("error|status|failed|delivery")) | "     log: \(.kind) \(.data | tostring | .[0:240])"' 2>/dev/null | head -n 12 || true
}

marker="kagent-e2e-$(uuid | cut -c1-8)"

# ===================================================================================================
echo "== message 1: kagent answers through the orchestrator"
thread=$(uuid)
echo "thread $thread (kagent), marker $marker"
code=$(run_message "$thread" "Say hello ($marker)" "$tmp/run1.sse")
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/kagent answered HTTP ${code:-none}: $(head -c 300 "$tmp/run1.sse.err") $(head -c 300 "$tmp/run1.sse")"
  finish
fi
sse_events "$tmp/run1.sse" > "$tmp/run1.json"
state=$(wait_state "$thread")
replay "$thread" "$tmp/events1.json"
said=$(said_of "$tmp/events1.json")
echo "kagent said: ${said:-<nothing>}"
if [ "$state" != "done" ]; then
  echo "     what the thread says about it:"
  why "$thread" "$tmp/events1.json"
  bad "the thread ended '$state' (want done) and the run ended '$(outcome_of "$tmp/run1.json")': the first message names no contextId (ADR 0055), so kagent has nothing to refuse; see what the thread says above"
  finish
fi

expect "the run stream starts (RUN_STARTED)" "$(jq -r '[.[] | select(.type == "RUN_STARTED")] | length > 0' "$tmp/run1.json")" "true"
expect "the run stream ends with RUN_FINISHED (success)" "$(outcome_of "$tmp/run1.json")" "success"
expect "the thread ended done (kagent answered, it did not wait)" "$state" "done"
case $said in
  *"kagent says hello"*) ok "the agent's message is the model's answer, 'kagent says hello'" ;;
  *) bad "the agent's message is '${said:-nothing}', want 'kagent says hello'" ;;
esac

# ===================================================================================================
echo "== the AG-UI stream and the log"
api GET "/api/threads/$thread/export" > "$tmp/export1.json" 2>/dev/null || echo '{}' > "$tmp/export1.json"
kinds=$(jq -r '[.events[]?.kind] | unique | join(" ")' "$tmp/export1.json")
note "the log's event kinds: $kinds"
expect "the log holds the agent's message" \
  "$(jq -r '[.events[]? | select(.kind == "agent_message")] | length > 0' "$tmp/export1.json")" "true"
# ADR 0055: the first message named no contextId, kagent assigned one, and the binding adopted it (export: binding.contextId)
bound=$(jq -r '.binding.contextId // empty' "$tmp/export1.json")
if [ -n "$bound" ] && [ "$bound" != "$thread" ]; then
  ok "the thread's binding adopted the context kagent assigned ($bound), not the thread id"
else
  bad "the thread's binding has context '${bound:-none}': want kagent's own, not the thread id $thread (ADR 0055)"
fi
note "the stream's frame types: $(jq -r '[.[].type] | unique | join(" ")' "$tmp/events1.json")"
steps=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step")] | length' "$tmp/events1.json")
note "steps in the thread: $steps (steps/v1 is an adam extension: kagent's card lists none, so its tool calls, if it made any, are not steps)"
caps=$(api GET /agui/agents/kagent/capabilities 2>/dev/null || echo '{}')
note "what the orchestrator says kagent can do beyond plain A2A (capabilities.custom): $(printf '%s' "$caps" | jq -c '(.custom // {}) | keys')"

# ===================================================================================================
echo "== message 2 in the same thread: kagent continues its conversation"
code=$(run_message "$thread" "And once more ($marker)" "$tmp/run2.sse")
if [ "$code" != 200 ]; then bad "the second POST answered HTTP ${code:-none}: $(head -c 300 "$tmp/run2.sse.err") $(head -c 300 "$tmp/run2.sse")"; fi
sse_events "$tmp/run2.sse" > "$tmp/run2.json"
state=$(wait_state "$thread")
replay "$thread" "$tmp/events2.json"
said=$(said_of "$tmp/events2.json")
echo "kagent said (the whole thread): ${said:-<nothing>}"
expect "the second run ends with RUN_FINISHED (success)" "$(outcome_of "$tmp/run2.json")" "success"
expect "the thread ended done again" "$state" "done"
case $said in
  *"kagent says hello again"*) ok "the second answer is the 'again' script's: the history of the first reached the model" ;;
  *) bad "the thread's words do not have 'kagent says hello again': ${said:-nothing}" ;;
esac

# kagent's own view: the tasks of this thread, by the marker the two messages carry, in how many contexts.
# `historyLength`: a task of a list has no history unless it is asked for, and the marker is in the message the person sent.
code=$(rpc ListTasks '{"historyLength": 50, "pageSize": 100}')
tasks=$(jq -c --arg m "$marker" '[((.result.tasks // .result // []) | if type == "array" then . else [] end)[]
  | select(tostring | contains($m))]' "$tmp/rpc.json" 2>/dev/null || echo '[]')
kctx=$(printf '%s' "$tasks" | jq -r '[.[].contextId] | first // empty')
export_now=$(api GET "/api/threads/$thread/export" 2>/dev/null | jq -r '.binding.contextId // empty' || true)
expect "the binding still holds the context of the first message after the second (one context for the thread)" "$export_now" "$bound"
if [ "$variant" = 1.x ]; then
  expect "kagent holds two tasks for the thread (ListTasks, HTTP $code)" "$(printf '%s' "$tasks" | jq 'length')" "2"
  expect "... in ONE context of its own" "$(printf '%s' "$tasks" | jq '[.[].contextId] | unique | length')" "1"
  expect "... which is the context the binding adopted" "$kctx" "$bound"
else
  # kagent 0.10's ListTasks on the v1 wire is not known to work (nothing here has run): recorded, not asserted
  finding "listTasks: kagent $variant answered HTTP $code to ListTasks with $(printf '%s' "$tasks" | jq 'length') tasks of the thread, in $(printf '%s' "$tasks" | jq '[.[].contextId] | unique | length') context(s)"
fi

# the model's journal: the requests of both messages, oldest first.
curl -s --max-time 30 "$model/__admin/requests" |
  jq -c '[.requests | reverse | .[].request | select(.url | test("chat/completions")) | .body | fromjson?]' > "$tmp/requests.json" 2>/dev/null || echo '[]' > "$tmp/requests.json"
expect "every model request was for the model of the ModelConfig, kagent-mock" \
  "$(jq -r '[.[] | select(.model != "kagent-mock")] | length' "$tmp/requests.json")" "0"
expect "the second message's request has the first answer in its history" \
  "$(jq -r '[.[] | select([.messages[] | select(.role == "assistant") | (.content // "")] | any(contains("kagent says hello")))] | length > 0' "$tmp/requests.json")" "true"
note "kagent's runtime called the model $(jq -r 'length' "$tmp/requests.json") times for the two messages, stream=$(jq -r '[.[].stream // false] | unique | join(",")' "$tmp/requests.json"), tools offered: $(jq -c '[.[0].tools[]?.function.name]' "$tmp/requests.json")"

if [ "$variant" = 1.x ]; then
  # ===================================================================================================
  echo "== human in the loop: a paused task"
  hthread=$(uuid)
  echo "thread $hthread (kagent)"
  code=$(run_message "$hthread" "[mock:ask] Which database should we use for storage? ($marker)" "$tmp/run3.sse")
  if [ "$code" != 200 ]; then bad "the HITL POST answered HTTP ${code:-none}: $(head -c 300 "$tmp/run3.sse.err") $(head -c 300 "$tmp/run3.sse")"; fi
  sse_events "$tmp/run3.sse" > "$tmp/run3.json"
  state=$(wait_state "$hthread")
  replay "$hthread" "$tmp/events3.json"
  said=$(said_of "$tmp/events3.json")
  echo "kagent said: ${said:-<nothing>}"
  expect "the run ends as an interrupt (the agent waits for the person)" "$(outcome_of "$tmp/run3.json")" "interrupt"
  expect "the thread ended blocked (an A2A input-required)" "$state" "blocked"
  case $said in
    *"Which database should we use?"*) ok "the question is the agent's message: a client that did not activate kagent's HITL extension gets its text" ;;
    *) bad "the agent's message is '${said:-nothing}', want the question 'Which database should we use?'" ;;
  esac
  case $(jq -c '[.capabilities.extensions[]?.uri]' "$tmp/card.json") in
    *"$hitl_uri"*) note "the card lists $hitl_uri; the orchestrator knows no such URI, so it never activates it (A2A-Extensions) and sends no structured answer" ;;
    *) note "the card does not list $hitl_uri: the runtime pauses without advertising the extension" ;;
  esac
  if [ "$state" = blocked ]; then
    # What the person's reply does: in words, on the same task, which kagent's adapter wants as a structured `ask_user_response`.
    code=$(run_message "$hthread" "PostgreSQL ($marker)" "$tmp/run4.sse")
    sse_events "$tmp/run4.sse" > "$tmp/run4.json"
    state2=$(wait_state "$hthread")
    replay "$hthread" "$tmp/events4.json"
    said2=$(said_of "$tmp/events4.json")
    finding "hitl: the person's answer in words (HTTP $code) ends the thread $state2, the run $(outcome_of "$tmp/run4.json"); the thread's words after it: $(printf '%s' "$said2" | sed "s/^.*Which database should we use?//" | head -c 300)"
    case $state2 in
      done | blocked | failed | cancelled) ok "the thread ends in a state ($state2), it does not hang" ;;
      *) bad "the thread did not end after the person's answer (state '$state2' after ${timeout}s)" ;;
    esac
    if [ "$state2" != "done" ]; then why "$hthread" "$tmp/events4.json"; fi
  fi

else
  note "no human-in-the-loop check against kagent $variant: the runtime of 0.10 is not scripted to call ask_user (dev/kagent/wiremock), the 1.x scenario has it"
fi

# ===================================================================================================
echo "== the model mock"
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "the model mock matched every request" "$unmatched" "0"

finish
