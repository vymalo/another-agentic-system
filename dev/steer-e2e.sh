#!/usr/bin/env sh
# System-level test of sending while an agent works (ADR 0036, steer/v1, plan 11 PR-16): a message sent to a running task is read by it at its
# next model turn, and Stop & send ends the task `canceled` within seconds and starts the next one from where the stopped one was. The agent is
# `chat`: `adam-agent` from the coder's pinned image (the same adam-rs commit as the coder: dev/coder/UPSTREAM; its card lists steer/v1) on the
# scripted `mock-persona`, whose `[mock:slow]` script (dev/wiremock/model/mappings/persona-slow*.json, ours) has two phases for a message that
# carries the keyword: at once a call of `ui_catalog` (a read-only tool step, no arguments), then, with its result in, an answer that takes 20 s
# (streamed over 20 s), so the task is `working` long enough to be steered or stopped. The tool step is there on purpose. adam reports a run as
# `submitted` until its FIRST COMMIT (adam-rs `crates/adam-a2a-runtime/src/convert.rs`), and during a task's first model call nothing has
# committed; the orchestrator logs `agent_status: working` only when the agent says so, and steers only a task the log has seen working
# (crates/app/src/dispatcher.rs; it keeps a steer sent right after a Stop & send out of the task being cancelled). A steer sent during the very
# first model call is therefore delivered after the turn, by design (ADR 0036, Built in PR-16): the first run of this script found it, with a
# 20 s first call. After the tool step has committed, the task is `working` while the second, slow model call is in flight, which is what this
# script steers and stops. The slow answer is streamed (text-stream/v1) so the agent also reports its words as they come. The coder's own model
# is adam-rs's vendored mock, which has no slow script and is never edited here; the orchestrator's side of steer/v1 and the adam backend's are
# the same for every agent.
# Nothing here talks to the agent: the script speaks AG-UI to the orchestrator through the edge and reads what the model was sent.
#
#   dev/steer-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; `chat` runs from the same image, with `adam-agent` and a
# folder, so there is nothing more to pull):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the web does (docs/api/agui.md, "Sending while an agent works"): one POST /agui/agents/{agentId} per message,
# and a message sent while a run is open is a second run that carries one new message and `forwardedProps["vymalo.send"]`: `"steer"` (Send) or
# `"interrupt"` (Stop & send). The thread's log is read from GET /api/threads/{id}/export, and the model's requests from the request journal
# of `mock-model` (WireMock), the way dev/tools-e2e.sh reads them.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the capabilities of `chat` list steer/v1 (the card lists it: the orchestrator uses it only then, ADR 0008);
#   * STEER, a thread whose first message is "[mock:slow] ..." (the tool step is done, the slow model call is in flight, the task `working` as the
#     orchestrator saw it), then a second run with "steer": the log has that message with `delivery: steer` and the model's next request, the one
#     the slow call's answer is followed by, carries it as its LAST message, once, with the first message still in front of it; the task read it
#     (three model requests in all: the tool call, the slow call, the steered turn, the last not slow), and it is ONE job:
#     no `job_started`, one `thread_state` that ends a job (`done`), so the message did not wait for the end of the turn; both runs end with RUN_FINISHED success;
#   * STOP & SEND, a new thread, the same first message, then a second run with "interrupt": the log has the message with `delivery: interrupt`,
#     the task ends `canceled` (an `agent_status` of the log) at most 5 s after that message, long before the 20 s model call would have ended
#     (the agent stops a call in flight: adam-rs #73); the next job is job 2 (`job_started`), the abandoned job is never judged (the only
#     `thread_state` that ends a job is the final `done`, after job 2) and job 2 ends `completed`;
#   * `referenceTaskIds`: job 2's task is told what the cancelled one was (the new message is the model's last, and the cancelled task's first
#     message is in front of it). That is the effect of the reference the orchestrator sends (the new task names the cancelled one in
#     `referenceTaskIds`, ADR 0021): the adam backend continues the run it references, and a task that names nothing starts from nothing, with
#     the new message alone (the wire itself is asserted by `cargo test -p orch-app --test stop_and_send` and `-p orch-agent-a2a --test steer`);
#   * `mock-model` matched every request.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL   http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   TIMEOUT          120    seconds to wait for a thread to finish
#
# It EMPTIES the request journal of `mock-model` before each part, so run it on a stack you are not in the middle of another scenario on. The
# whole script takes about a minute (the steered task waits for its 20 s second model call). Needs curl and jq (and /proc or uuidgen for a UUID).
# Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
timeout=${TIMEOUT:-120}

steer_uri=https://agents.vymalo.com/a2a/extensions/steer/v1
slow_text='[mock:slow] Refactor the parser, and take your time.'
steer_text='you were wrong since line 1'
next_text='forget the parser, rename the crate instead'

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "steer e2e passed"; else echo "steer e2e FAILED"; exit 1; fi
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

# start_run OUT AGENT THREAD TEXT [FORWARDED_PROPS_JSON]: one AG-UI run (a message) on the thread, in the background. The SSE response is saved in OUT
# (a run stays open until its job ends, or until a message that is sent while it is open ends it), the HTTP status in OUT.code once it is over.
start_run() {
  _props=${5:-}
  [ -n "$_props" ] || _props='{}'
  _input=$(jq -n --arg thread "$3" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$4" --argjson props "$_props" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
  (
    curl -sS -N --max-time "$timeout" -o "$1" -w '%{http_code}' -X POST "$base/agui/agents/$2" -H "$id_header" \
      -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" >"$1.code" 2>"$1.err" || true
  ) &
  run_pid=$!
}

run_outcome() { # run_outcome OUT: how the run saved in OUT ended (success, interrupt, error: <code>, or empty)
  sed -n 's/^data: *//p' "$1" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true
}

# why_not OUT: what a run that did not start said (its HTTP status and body), for a failure message.
why_not() { echo "HTTP $(cat "$1.code" 2>/dev/null || echo '?') $(head -c 300 "$1" 2>/dev/null) $(head -c 300 "$1.err" 2>/dev/null)"; }

# waitfor SECONDS COMMAND [ARG...]: runs the command each second until it succeeds (0), or until SECONDS have passed (1).
waitfor() {
  _until=$(( $(date +%s) + $1 ))
  shift
  while ! "$@"; do
    [ "$(date +%s)" -lt "$_until" ] || return 1
    sleep 1
  done
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

# export_has THREAD JQ_EXPRESSION: whether the thread's export (the whole log) satisfies the jq expression.
export_has() {
  api GET "/api/threads/$1/export" 2>/dev/null | jq -e "$2" >/dev/null 2>&1
}

# requests: the bodies of the requests the model mock got for `mock-persona`, as a JSON array, the oldest first (WireMock's journal lists the
# newest first).
requests() {
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c '[.requests | reverse | .[].request.body | fromjson? | select(.model == "mock-persona")]' 2>/dev/null || echo '[]'
}

# slow_seen: whether the model mock got the slow request, the second phase of `[mock:slow]`: the one that ends with the tool's result, in a
# conversation that says `[mock:slow]`. The tool step before it is done (committed), and the slow model call is in flight.
slow_seen() {
  requests | jq -e 'any(.[]; .messages[-1].role == "tool" and any(.messages[]; .role == "user" and ((.content // "") | tostring | contains("[mock:slow]"))))' >/dev/null 2>&1
}

reset_journal() { # reset_journal: the mock's request journal starts empty
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
  if [ "$_code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $_code"; fi
}

# working_seen THREAD: whether the log says the task works, which is when the orchestrator knows it can be steered.
working_seen() {
  export_has "$1" 'any(.events[]; .kind == "agent_status" and .data.status == "working")'
}

# --- the stack, and the agent the scenario needs ------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" chat "*) ;;
  *) echo "the agent 'chat' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac

echo "== what the agent says it can do, before anyone sends"
chat_caps=$(api GET /agui/agents/chat/capabilities 2>/dev/null || echo '{}')
expect "the capabilities of chat list steer/v1 (its card lists it: a message to its running task is read by it)" \
  "$(printf '%s' "$chat_caps" | jq -r --arg u "$steer_uri" '(.custom // {}) | has($u)')" "true"

# ===================================================================================================
echo "== STEER: a message sent while the task works is read by the running task"
reset_journal
thread=$(uuid)
echo "thread $thread (chat)"
start_run "$tmp/steer1.sse" chat "$thread" "$slow_text"
run1=$run_pid
if waitfor 60 slow_seen; then ok "the tool step is done and the slow model call is in flight (the mock got the slow request)"; else bad "the model mock never got the slow request (the one that follows the tool step): $(why_not "$tmp/steer1.sse")"; fi
if waitfor 60 working_seen "$thread"; then ok "the log says the task works (the tool step committed)"; else bad "the log never said the task works"; fi

start_run "$tmp/steer2.sse" chat "$thread" "$steer_text" '{"vymalo.send":"steer"}'
run2=$run_pid
if waitfor 30 export_has "$thread" 'any(.events[]; .kind == "user_message" and .data.delivery == "steer")'; then
  ok "the second message is in the log with delivery: steer"
else
  bad "the second run did not log a message with delivery: steer: $(why_not "$tmp/steer2.sse")"
fi
state=$(wait_state "$thread" "done")
expect "the thread ends done" "$state" "done"
wait "$run1" || true
wait "$run2" || true
expect "the run that was open ends with RUN_FINISHED success (a message ends the run, not the thread)" "$(run_outcome "$tmp/steer1.sse")" "success"
expect "the run of the steered message ends with RUN_FINISHED success" "$(run_outcome "$tmp/steer2.sse")" "success"

api GET "/api/threads/$thread/export" >"$tmp/steer.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/steer.export.json"
expect "the steered message is the person's words, as sent" \
  "$(jq -r '[.events[] | select(.kind == "user_message" and .data.delivery == "steer") | .data.text] | join("|")' "$tmp/steer.export.json")" "$steer_text"
expect "it is one job: no job_started (the message did not wait for the end of the turn, and start another)" \
  "$(jq -r '[.events[] | select(.kind == "job_started")] | length' "$tmp/steer.export.json")" "0"
expect "and one end: a single thread_state that ends a job, done" \
  "$(jq -r '[.events[] | select(.kind == "thread_state" and (.data.state == "done" or .data.state == "failed" or .data.state == "cancelled")) | .data.state] | join(",")' "$tmp/steer.export.json")" "done"
expect "the task ends completed" \
  "$(jq -r '[.events[] | select(.kind == "agent_status") | .data.status] | last' "$tmp/steer.export.json")" "completed"

requests >"$tmp/steer.requests.json"
echo "the model got $(jq -r 'length' "$tmp/steer.requests.json") requests; the last one ended with: $(jq -r '.[-1].messages[-1].content // "<nothing>"' "$tmp/steer.requests.json" | head -c 200)"
expect "the model got three requests (the tool call, the slow call, then the turn the steered message made)" "$(jq -r 'length' "$tmp/steer.requests.json")" "3"
expect "the first one ended with the first message, the one that asks for the tool step" \
  "$(jq -r '.[0].messages[-1] | [.role, ((.content // "") | tostring | contains("[mock:slow]"))] | join(" ")' "$tmp/steer.requests.json")" "user true"
expect "the second one is the slow call: it ends with the tool's result" \
  "$(jq -r '.[1].messages[-1].role' "$tmp/steer.requests.json")" "tool"
expect "the third one ends with the steered message: the model's next request quotes it" \
  "$(jq -r --arg t "$steer_text" '.[2].messages[-1] | [.role, (.content | tostring | contains($t))] | join(" ")' "$tmp/steer.requests.json")" "user true"
expect "and holds it once" \
  "$(jq -r --arg t "$steer_text" '[.[2].messages[] | select((.content // "") | tostring | contains($t))] | length' "$tmp/steer.requests.json")" "1"
expect "the first message is still in front of it (the same task went on)" \
  "$(jq -r '[.[2].messages[] | select((.content // "") | tostring | contains("[mock:slow]"))] | length' "$tmp/steer.requests.json")" "1"
expect "and so is the tool step (the call and its result)" \
  "$(jq -r '[.[2].messages[] | select(.role == "tool" or ((.tool_calls // []) | length) > 0)] | length' "$tmp/steer.requests.json")" "2"

# ===================================================================================================
echo "== STOP & SEND: the task is cancelled in seconds and the next one starts from where it was"
reset_journal
thread=$(uuid)
echo "thread $thread (chat)"
start_run "$tmp/stop1.sse" chat "$thread" "$slow_text"
run1=$run_pid
if waitfor 60 slow_seen; then ok "the tool step is done and the slow model call is in flight (the mock got the slow request)"; else bad "the model mock never got the slow request (the one that follows the tool step): $(why_not "$tmp/stop1.sse")"; fi
if waitfor 60 working_seen "$thread"; then ok "the log says the task works (the tool step committed)"; else bad "the log never said the task works"; fi

start_run "$tmp/stop2.sse" chat "$thread" "$next_text" '{"vymalo.send":"interrupt"}'
run2=$run_pid
if waitfor 30 export_has "$thread" 'any(.events[]; .kind == "user_message" and .data.delivery == "interrupt")'; then
  ok "the second message is in the log with delivery: interrupt"
else
  bad "the second run did not log a message with delivery: interrupt: $(why_not "$tmp/stop2.sse")"
fi
# The 20 s model call is in flight; a stop that works ends the task long before it would have.
if waitfor 25 export_has "$thread" 'any(.events[]; .kind == "agent_status" and .data.status == "canceled")'; then
  ok "the task ended canceled"
else
  bad "the task never ended canceled (the stop did not reach the model call in flight?)"
fi
state=$(wait_state "$thread" "done")
expect "the thread ends done (job 2 ran to its end)" "$state" "done"
wait "$run1" || true
wait "$run2" || true
expect "the run that was open ends with RUN_FINISHED success" "$(run_outcome "$tmp/stop1.sse")" "success"
expect "the run of the message ends with RUN_FINISHED success" "$(run_outcome "$tmp/stop2.sse")" "success"

api GET "/api/threads/$thread/export" >"$tmp/stop.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/stop.export.json"
# The events carry the orchestrator's own clock (`at`), so the time is the stop's, not the poll's.
# shellcheck disable=SC2016 # jq's own variables, not the shell's
seconds=$(jq -r 'def ts: (sub("\\.[0-9]+"; "") | sub("\\+00:00$"; "Z") | fromdateiso8601) + ((capture("\\.(?<f>[0-9]+)") // {f: "0"}).f | ("0." + .) | tonumber);
  ([.events[] | select(.kind == "user_message" and .data.delivery == "interrupt") | .at | ts] | first) as $sent
  | ([.events[] | select(.kind == "agent_status" and .data.status == "canceled") | .at | ts] | first) as $stopped
  | if $sent == null or $stopped == null then "none" else ($stopped - $sent) | tostring end' "$tmp/stop.export.json" 2>/dev/null || echo none)
echo "the stop took ${seconds}s from the message to the canceled status"
case $seconds in
  none) bad "the log has no interrupting message followed by a canceled status" ;;
  *) expect "the task ended canceled within 5 s of the message" \
       "$(awk -v d="$seconds" 'BEGIN { print (d >= 0 && d <= 5) ? "within 5 s" : "outside 5 s" }')" "within 5 s" ;;
esac
# shellcheck disable=SC2016
order=$(jq -r '[.events[] | select((.kind == "user_message" and .data.delivery == "interrupt") or (.kind == "agent_status" and .data.status == "canceled") or .kind == "job_started"
    or (.kind == "agent_status" and .data.status == "completed")
    or (.kind == "thread_state" and (.data.state == "done" or .data.state == "failed" or .data.state == "cancelled")))
  | if .kind == "user_message" then "interrupt" elif .kind == "agent_status" then .data.status elif .kind == "job_started" then "job \(.data.job)" else "thread \(.data.state)" end] | join(", ")' "$tmp/stop.export.json")
expect "the order: the message, the task canceled, job 2, its task completed, and only then the thread done (the abandoned job is never judged)" \
  "$order" "interrupt, canceled, job 2, completed, thread done"
expect "the next job's message is the person's words, as sent" \
  "$(jq -r '[.events[] | select(.kind == "user_message" and .data.delivery == "interrupt") | .data.text] | join("|")' "$tmp/stop.export.json")" "$next_text"

requests >"$tmp/stop.requests.json"
echo "the model got $(jq -r 'length' "$tmp/stop.requests.json") requests; the last one ended with: $(jq -r '.[-1].messages[-1].content // "<nothing>"' "$tmp/stop.requests.json" | head -c 200)"
expect "the last request is job 2's: it ends with the new message, once" \
  "$(jq -r --arg t "$next_text" '[.[] | select((.messages[-1].content // "") | tostring | contains($t))] | length' "$tmp/stop.requests.json")" "1"
expect "the cancelled task's first message is in front of it (job 2 continues the task it names in referenceTaskIds: ADR 0021)" \
  "$(jq -r --arg t "$next_text" '[.[] | select((.messages[-1].content // "") | tostring | contains($t)) | .messages[] | select((.content // "") | tostring | contains("[mock:slow]"))] | length' "$tmp/stop.requests.json")" "1"

# --- nothing off-script ---------------------------------------------------------------------------------------------
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "mock-model matched every request" "$unmatched" "0"

finish
