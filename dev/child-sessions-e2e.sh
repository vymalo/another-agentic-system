#!/usr/bin/env sh
# System-level test of child sessions (child-sessions/v1, ADR 0072, docs/api/child-sessions-v1.md): a helper an agent runs as a session is a
# thread of its own, nested under the parent's, whose log is the helper's round; the parent's log says that it opened, reported and closed,
# once each; the helper's tokens are logged and not counted, so the month holds the parent's totals alone; and the AG-UI stream draws the
# session as a subagent and a card in the parent's run.
#
#   dev/child-sessions-e2e.sh
#
# Start the `app` profile first (the mock issuer is built with the user below, so build it):
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-sessions` (dev/agents.yaml), a WireMock A2A agent (dev/wiremock/sessions, ours) that serves the extension as adam does
# (adam-a2a-runtime `subscribe.rs`, `session_events.rs`): its card lists child-sessions/v1 (the helper `planner`, `maxOpen` 4), steps/v1 and
# usage/v1. A request streams, over two seconds, a first Task that says its sessions (none yet), `working`, the event `opened` (round 1
# working), the step "Start planner", the helper's call report `h1` (glm-5.3-mini, under that step), the step done, the parent's call report
# `p1` (glm-5.3) and `completed`. The helper is a background session: it still works when the parent ends. `GetTask` of the parent, read before
# the status that ends the task, says its totals (glm-5.3 1000 in, 100 out; glm-5.3-mini 400 in, 50 out: adam counts a round its model started in the
# parent's totals) and the session `<task>-s1` working, round 1 working. The round (the task `<task>-s1`) is held for 2.5 s by the mock, then
# served by `SubscribeToTask` (a step, one call report of glm-5.3-mini, `completed`, its own object saying the session closed) and by `GetTask`
# (its totals: glm-5.3-mini 400 in, 50 out) to a request that activates the extension, and refused with InvalidParams to one that does not. So the
# parent ends first, and the round's own object reports and closes the session. The parent's `contextId` is one value (`ctx-<task>`) in the stream,
# in `GetTask` and in the round's `parentContextId`. The word `[mock:sessions-poll]` makes the mock say no sessions object and no event (the Rust test
# `the_sessions_mock_with_no_events_*` runs that way: the orchestrator reads the task). The script runs as `child-sessions@example.com`
# (dev/mock-oidc/users.json), which no other scenario uses, and reads that user's month before and after, so a second run on the same database passes too.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the parent thread ends `done` and so does the child; the parent's log holds `child_session_opened` while it works, its own `completed`,
#     then `child_session_reported` (round 1 `done`) and `child_session_closed`, once each, naming the same child thread and session (the helper
#     `planner`, its description, not kept open, by the agent);
#   * there is exactly one child thread of the parent (`childOf.threadId`), nested under it (`nestedUnder`, and in the person's list), bound to the
#     agent `mock-sessions`, `childOf` naming the session, the helper and its state `closed`; its title is the helper's description (the
#     orchestrator's own title model never titles a child) and it has no `thread_titled` or `thread_described`;
#   * the child's log is the round and nothing the person sent: `job_started` (job 1, round 1) first, the helper's step, the call report,
#     the totals, `completed` and `done`, no `user_message`, with consecutive `seq`; the call report and the totals say `counted: false`, and the
#     parent's say nothing of it (they count);
#   * the agent was sent one message (the parent's) and nothing for the child: the methods it got are exactly SendStreamingMessage (once),
#     GetTask and SubscribeToTask; the message activated the extension by the header and by `message.extensions`, every `GetTask` and
#     `SubscribeToTask` activated it, the parent task was read at least once and the round was subscribed to by its task (`<task>-s1`) only;
#   * `GET /api/me/usage` counts exactly one more task, glm-5.3 1000 in, 100 out, 1100 more and glm-5.3-mini 400 in, 50 out, 450 more, written by hand
#     from the mock: the helper's own totals (`counted: false`, 450 again) are not counted a second time, which would make 900;
#   * the parent's AG-UI stream holds the subagent `planner` (`sub-session-<seq>`, metadata `vymalo.childThread` and `vymalo.session`), suspended
#     when the parent's run ends with the helper still working, started again (`sub-session-<seq>-<seq>`) when the round reports and finished with
#     the report; one `vymalo.session` card on one message id says `working`, then `idle` with the report `done`, then `closed`; the run's
#     `RUN_FINISHED.usage` is the parent's totals;
#   * the child's AG-UI stream has the `vymalo.job` card with the round and its usage frames say `counted: false`.
# Exit status 0 when every check passed.
#
# *Verified 2026-11-19*: run twice, with `dev/e2e-all.sh child-sessions usage` and `dev/check-mocks.sh`, against the real orchestrator (a debug build run in
# Docker), Postgres, `mock-oidc` and the WireMock agents of this stack; the edge, `web` and the real agents were not run there. CI runs it in the full stack.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL           http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL         child-sessions@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_SESSIONS_URL  http://127.0.0.1:${MOCK_SESSIONS_PORT:-8102}   the agent (WireMock admin API)
#   TIMEOUT            120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-sessions` first, so run it on a stack you are not in the middle of another scenario on. A task is
# counted in the month of the latest event that changed it (ADR 0065), so a run that crosses the end of a UTC month fails the usage check: run
# it again. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml; the orchestrator side
# against the same WireMock agent is also `the_sessions_mock_*` of orchestrator/crates/e2e/tests/wiremock_agent.rs.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-child-sessions@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
mock=${MOCK_SESSIONS_URL:-http://127.0.0.1:${MOCK_SESSIONS_PORT:-8102}}
mock=${mock%/}
timeout=${TIMEOUT:-120}

sessions_uri=https://agents.vymalo.com/a2a/extensions/child-sessions/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "child sessions e2e passed"; else echo "child sessions e2e FAILED"; exit 1; fi
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

# run_agui AGENT THREAD TEXT: one run (a message) on a new thread, to its end. Prints the HTTP status.
run_agui() {
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

# wait_state THREAD: waits (up to TIMEOUT seconds) until the thread stops, and prints the state it ended in (or the last one it saw).
wait_state() {
  if [ -z "$1" ]; then echo "none"; return 0; fi
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(api GET "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' || true)
    case $_state in done | blocked | failed | cancelled) echo "$_state"; return 0 ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then echo "${_state:-unknown}"; return 0; fi
    sleep 2
  done
}

# frames THREAD FILE: the thread's AG-UI events, replayed from the start, as one JSON array in FILE.
frames() {
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' "$base/agui/threads/$1/connect?mode=run" 2>/dev/null |
    sed -n 's/^data: *//p' | jq -s '.' >"$2" 2>/dev/null || echo '[]' >"$2"
}

# month FILE: the person's month (GET /api/me/usage) in FILE.
month() {
  api GET /api/me/usage >"$1" 2>/dev/null || echo '{}' >"$1"
}

# calls METHOD: for each JSON-RPC call of METHOD the mock got, `<activated>|<task id>` (whether the A2A-Extensions header names
# child-sessions/v1, and the task the call named), one line each (WireMock's journal; a header name in any case).
calls() {
  curl -s --max-time 30 "$mock/__admin/requests" | jq -r --arg m "$1" --arg u "$sessions_uri" '
    .requests[] | .request as $r | ($r.body | fromjson? // {}) as $b
    | select($b.method == $m)
    | ([$r.headers | to_entries[] | select(.key | ascii_downcase == "a2a-extensions") | .value
        | if type == "array" then join(",") else tostring end] | join(",") | contains($u)) as $a
    | "\($a)|\($b.params.id // "")"' 2>/dev/null || true
}

# --- the stack ---------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" mock-sessions "*) ;;
  *) echo "the agent 'mock-sessions' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac
ok "GET /api/agents lists mock-sessions"
_code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$mock/__admin/requests" || true)
if [ "$_code" = 200 ]; then ok "journal reset: $mock"; else bad "journal reset: $mock answered HTTP $_code"; fi
month "$tmp/month.before.json"
if [ "$(jq -r '.currency // empty' "$tmp/month.before.json")" = "" ]; then
  echo "GET /api/me/usage did not answer a month for $email: is the orchestrator a build with the usage ledger, ADR 0065, and $email a user of dev/mock-oidc/users.json (rebuild mock-oidc)?" >&2
  exit 2
fi
nonce=$(uuid)

# --- the parent --------------------------------------------------------------------------------------------------------
echo "== mock-sessions: a parent task that opens one session and ends before it does"
parent=$(uuid)
code=$(run_agui mock-sessions "$parent" "plan the migration ($nonce)")
expect "POST /agui/agents/mock-sessions answers 200" "$code" "200"
expect "the parent thread ends done" "$(wait_state "$parent")" "done"
# The stack's title and description models write after the job: the log is read once they have, so the stream below has the order CI met.
# A stack without them goes on after 15 s.
_deadline=$(( $(date +%s) + 15 ))
while :; do
  _after=$(api GET "/api/threads/$parent/export" 2>/dev/null |
    jq -r '[.events[] | select(.kind == "thread_titled" or .kind == "thread_described") | .kind] | unique | length' 2>/dev/null || echo 0)
  if [ "$_after" = 2 ] || [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
  sleep 1
done
echo "     the orchestrator's own title and description after the job: $_after of 2"
# The helper works on after the parent ended: its round reports and closes the session some seconds later.
_deadline=$(( $(date +%s) + timeout ))
while :; do
  api GET "/api/threads/$parent/export" >"$tmp/parent.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/parent.export.json"
  _closed=$(jq -r '[.events[] | select(.kind == "child_session_closed")] | length' "$tmp/parent.export.json" 2>/dev/null || echo 0)
  if [ "$_closed" -ge 1 ] || [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
  sleep 1
done

expect "the parent's log: opened while it works, its own completed, then the round's report and the close, once each" \
  "$(jq -r '[.events[] | select(.kind == "child_session_opened" or .kind == "child_session_reported" or .kind == "child_session_closed"
     or (.kind == "agent_status" and .data.status == "completed")) | if .kind == "agent_status" then "completed" else .kind end] | join(",")' "$tmp/parent.export.json")" \
  "child_session_opened,completed,child_session_reported,child_session_closed"
child=$(jq -r '[.events[] | select(.kind == "child_session_opened") | .data.thread] | first // ""' "$tmp/parent.export.json")
session=$(jq -r '[.events[] | select(.kind == "child_session_opened") | .data.session] | first // ""' "$tmp/parent.export.json")
task=$(jq -r '[.events[] | select(.kind == "model_usage") | .data.task] | first // ""' "$tmp/parent.export.json")
expect "the session is the agent's: the id of the round, the parent task's id and -s1" "$session" "$task-s1"
expect "opened: the helper planner, its description, not kept open, round 1" \
  "$(jq -r '[.events[] | select(.kind == "child_session_opened") | .data | "\(.agent) | \(.description) | \(.keepOpen) | \(.round)"] | first // ""' "$tmp/parent.export.json")" \
  "planner | Plan the migration | false | 1"
expect "reported: round 1 done, and closed, each naming the same session and the same child thread" \
  "$(jq -r --arg c "$child" --arg s "$session" '[.events[] | select(.kind == "child_session_reported" or .kind == "child_session_closed")
     | select(.data.thread == $c and .data.session == $s) | if .kind == "child_session_reported" then "reported:\(.data.round):\(.data.report)" else "closed" end] | join(",")' "$tmp/parent.export.json")" \
  "reported:1:done,closed"
if [ -n "$child" ]; then ok "the parent names a child thread"; else bad "the parent names no child thread"; fi

# --- the child ----------------------------------------------------------------------------------------------------------
echo "== the child thread"
expect "the child thread ends done" "$(wait_state "$child")" "done"
api GET "/api/threads/$child" >"$tmp/child.json" 2>/dev/null || echo '{}' >"$tmp/child.json"
expect "childOf names the parent, the session, the helper and its state closed" \
  "$(jq -r '.childOf | "\(.threadId) | \(.session) | \(.agent) | \(.state)"' "$tmp/child.json")" "$parent | $session | planner | closed"
expect "it is nested under the parent and bound to the agent mock-sessions" \
  "$(jq -r '"\(.nestedUnder) | \(.target.agentId)"' "$tmp/child.json")" "$parent | mock-sessions"
expect "its title is the helper's description" "$(jq -r '.title' "$tmp/child.json")" "Plan the migration"
api GET "/api/threads?order=rail" >"$tmp/rail.json" 2>/dev/null || echo '[]' >"$tmp/rail.json"
expect "the person's list has exactly one child of the parent, nested under it" \
  "$(jq -r --arg p "$parent" '[.[] | select(.childOf.threadId == $p and .nestedUnder == $p) | .id] | join(",")' "$tmp/rail.json")" "$child"
expect "the parent is a top-level thread of the list: not nested, no childOf" \
  "$(jq -r --arg p "$parent" '[.[] | select(.id == $p) | "\(.nestedUnder) \(.childOf)"] | first // ""' "$tmp/rail.json")" "null null"

api GET "/api/threads/$child/export" >"$tmp/child.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/child.export.json"
expect "the child's log is the round: the job nobody sent, the step, the call, the step done, the totals, completed, done" \
  "$(jq -r '[.events[] | if .kind == "agent_status" or .kind == "thread_state" then "\(.kind):\(.data.status // .data.state)" else .kind end] | join(",")' "$tmp/child.export.json")" \
  "job_started,agent_step,model_usage,agent_step,model_usage_total,agent_status:completed,thread_state:done"
expect "it starts with job 1, round 1, and holds nothing the person sent" \
  "$(jq -r '[(.events[0].data | "\(.job) \(.round.number)"), ([.events[] | select(.kind == "user_message")] | length | tostring)] | join(" | ")' "$tmp/child.export.json")" \
  "1 1 | 0"
expect "its seq are consecutive from 1" \
  "$(jq -r '[.events[].seq] == [range(1; (.events | length) + 1)]' "$tmp/child.export.json")" "true"
expect "the orchestrator's own title model never titled or described it" \
  "$(jq -r '[.events[] | select(.kind == "thread_titled" or .kind == "thread_described")] | length' "$tmp/child.export.json")" "0"
expect "the helper's call and totals are logged and not counted (counted: false), the model its own: glm-5.3-mini" \
  "$(jq -r '[.events[] | select(.kind == "model_usage" or .kind == "model_usage_total") | "\(.kind) \(.data.counted) \(.data.call // "-") \(.data.model // (.data.totals | map(.model) | join(",")))"] | join(" | ")' "$tmp/child.export.json")" \
  "model_usage false h1 glm-5.3-mini | model_usage_total false - glm-5.3-mini"
expect "the parent's own calls and totals say nothing of it: they count (the helper's call h1 and the parent's p1, then the totals of both models)" \
  "$(jq -r '[.events[] | select(.kind == "model_usage" or .kind == "model_usage_total") | "\(.kind) \(.data | has("counted")) \(.data.call // "-") \(.data.model // (.data.totals | map(.model) | join(",")))"] | join(" | ")' "$tmp/parent.export.json")" \
  "model_usage false h1 glm-5.3-mini | model_usage false p1 glm-5.3 | model_usage_total false - glm-5.3,glm-5.3-mini"

# --- what the agent was asked ---------------------------------------------------------------------------------------------
echo "== what mock-sessions was asked: the parent's message, activated; the round by task, and nothing sent for the child"
sends=$(curl -s --max-time 30 "$mock/__admin/requests" | jq -r --arg n "$nonce" --arg u "$sessions_uri" '
  [.requests[] | .request as $r | ($r.body | fromjson? // {}) as $b
   | select($b.method == "SendStreamingMessage")
   | ([$r.headers | to_entries[] | select(.key | ascii_downcase == "a2a-extensions") | .value
       | if type == "array" then join(",") else tostring end] | join(",") | contains($u)) as $h
   | "\($h)|\(($b.params.message.extensions // []) | index($u) != null)|\([$b.params.message.parts[]?.text // ""] | join(" ") | contains($n))"] | join(",")' 2>/dev/null || true)
expect "one message was sent to the agent in all, the parent's, activated by the header and by the message" "$sends" "true|true|true"
get_calls=$(calls GetTask)
subscribes=$(calls SubscribeToTask)
_reads=$(printf '%s\n' "$get_calls" | grep -cx "true|$task" || true)
if [ "$_reads" -ge 1 ]; then ok "the parent task was read for its sessions (GetTask $task, activated: $_reads)"; else bad "the parent task was not read (GetTask $task, activated)"; fi
expect "every GetTask and SubscribeToTask the agent got activated the extension (none was refused)" \
  "$(printf '%s\n%s\n' "$get_calls" "$subscribes" | grep -c '^false|' || true)" "0"
expect "the round was subscribed to by its task, $task-s1, and nothing else was" \
  "$(printf '%s\n' "$subscribes" | grep -v '^$' | sort -u | paste -sd, -)" "true|$task-s1"
expect "the agent got these methods and no others: SendStreamingMessage (the parent's), GetTask and SubscribeToTask: nothing was sent for the child" \
  "$(curl -s --max-time 30 "$mock/__admin/requests" | jq -r '[.requests[] | (.request.body | fromjson? // {}) | .method // empty] | unique | join(",")' 2>/dev/null)" \
  "GetTask,SendStreamingMessage,SubscribeToTask"
expect "and the one SendStreamingMessage is the only message" \
  "$(curl -s --max-time 30 "$mock/__admin/requests" | jq -r '[.requests[] | (.request.body | fromjson? // {}) | select(.method == "SendStreamingMessage")] | length' 2>/dev/null)" "1"

# --- the month ------------------------------------------------------------------------------------------------------------
echo "== GET /api/me/usage counts the parent's totals, which hold the helper's tokens, once"
month "$tmp/month.after.json"
expect "one more task" \
  "$(jq -rn --slurpfile a "$tmp/month.after.json" --slurpfile b "$tmp/month.before.json" '($a[0].tasks // 0) - ($b[0].tasks // 0)')" "1"
# The constants are the mock's, by hand: glm-5.3 1000 in, 100 out, 1100 (the parent's own call); glm-5.3-mini 400 in, 50 out, 450 (the helper's, in the
# parent's totals). The helper's own totals say glm-5.3-mini 450 again with `counted: false`: counting them too would make it 900.
expect "glm-5.3: 1000 in, 100 out, 1100 more; glm-5.3-mini: 400 in, 50 out, 450 more; no other model" \
  "$(jq -rn --slurpfile a "$tmp/month.after.json" --slurpfile b "$tmp/month.before.json" '
     def at($d; $m): [($d.models // [])[] | select(.model == $m)] | first // {inputTokens: 0, outputTokens: 0, totalTokens: 0};
     def delta($m): at($a[0]; $m) as $x | at($b[0]; $m) as $y | "\($x.inputTokens - $y.inputTokens)/\($x.outputTokens - $y.outputTokens)/\($x.totalTokens - $y.totalTokens)";
     "\(delta("glm-5.3")) \(delta("glm-5.3-mini")) \([($a[0].models // [])[] | .model] | map(select(. != "glm-5.3" and . != "glm-5.3-mini")) | length)"')" \
  "1000/100/1100 400/50/450 0"

# --- AG-UI ----------------------------------------------------------------------------------------------------------------
echo "== AG-UI: the session is a subagent and a card in the parent's run, a job with a round in the child's"
frames "$parent" "$tmp/parent.frames.json"
expect "two SUBAGENT_STARTED named planner (sub-session-<seq>, then sub-session-<seq>-<seq>), each carrying the child thread and the session" \
  "$(jq -r '[.[] | select(.type == "SUBAGENT_STARTED" and .name == "planner") | "\(.subagentRunId | test("^sub-session-[0-9]+(-[0-9]+)?$")) \(.metadata["vymalo.childThread"]) \(.metadata["vymalo.session"])"] | join(",")' "$tmp/parent.frames.json")" \
  "true $child $session,true $child $session"
expect "the first subagent is suspended when the parent's run ends with the helper working, the second finishes with the report done on round 1" \
  "$(jq -r '. as $all | [.[] | select(.type == "SUBAGENT_STARTED" and .name == "planner") | .subagentRunId] as $ids
     | [$all[] | select(.type == "SUBAGENT_FINISHED" and (.subagentRunId == $ids[0] or .subagentRunId == $ids[1]))
        | "\(.subagentRunId == $ids[0]) \(.outcome.type // "-") \(.result.report // "-") \(.result.round // "-")"] | join(",")' "$tmp/parent.frames.json")" \
  "true suspended - -,false - done 1"
expect "three vymalo.session cards on one message id: working, idle with the report done, closed" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.session")] | "\(map(.messageId) | unique | length) \(map(.content.state) | join(",")) \(map(.content.report // "-") | join(","))"' "$tmp/parent.frames.json")" \
  "1 working,idle,closed -,done,done"
expect "the last card says the thread, the helper, the description, not kept open, round 1 and the report" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.session")] | last | .content | "\(.threadId) | \(.session) | \(.agent) | \(.description) | \(.keepOpen) | \(.round) | \(.report)"' "$tmp/parent.frames.json")" \
  "$child | $session | planner | Plan the migration | false | 1 | done"
expect "the run's RUN_FINISHED.usage is the parent's totals: glm-5.3 1000 in, 100 out, 1100 and glm-5.3-mini 400 in, 50 out, 450" \
  "$(jq -r '[.[] | select(.type == "RUN_FINISHED" and has("usage")) | .usage | map("\(.model):\(.inputTokens):\(.outputTokens):\(.totalTokens)") | sort | join(" ")] | join(",")' "$tmp/parent.frames.json")" \
  "glm-5.3-mini:400:50:450 glm-5.3:1000:100:1100"

frames "$child" "$tmp/child.frames.json"
expect "the child's stream: the vymalo.job card says job 1 and round 1" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.job") | "\(.content.job) \(.content.round.number)"] | join(",")' "$tmp/child.frames.json")" "1 1"
expect "the child's stream: its usage frames, the call and the totals, say counted false" \
  "$(jq -r '[.[] | select(.type == "CUSTOM" and (.name | startswith("vymalo.usage"))) | "\(.name):\(.value.counted)"] | join(",")' "$tmp/child.frames.json")" \
  "vymalo.usage:false,vymalo.usage_total:false"

finish
