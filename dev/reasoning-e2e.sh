#!/usr/bin/env sh
# System-level test of a model's reasoning on its way to the screen (ADR 0044, adam-rs ADR 0020): the agent's model writes `reasoning_content`
# before its answer, the agent sends it as `text-stream/v1` chunks marked `kind: "reasoning"`, the orchestrator relays them live and logs the
# reasoning once, and the AG-UI stream carries it as a reasoning span before the words of the turn. The agent is `chat`: `adam-agent` from the
# coder's pinned image on the scripted `mock-persona`, whose `[mock:think]` script (dev/wiremock/model/mappings/persona-think*.json, ours) streams
# three pieces of `reasoning_content` and then three of the answer.
#
#   dev/reasoning-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; `chat` runs from the same image):
#
#   docker compose --profile app up -d --build --wait
#
# The pinned image must be at an adam-rs revision that has ADR 0020 (dev/coder/UPSTREAM): an older agent sends no reasoning, and the script says so
# in its first failing check rather than failing on something later.
#
# The script speaks AG-UI, as the web does (docs/api/agui.md, "Reasoning"): one POST /agui/agents/chat with a message that carries `[mock:think]`.
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the capabilities of `chat` list text-stream/v1 (the orchestrator reads reasoning only from an agent that lists it, ADR 0008);
#   * the run ends with RUN_FINISHED success and the thread `done`;
#   * the run stream has the five reasoning events of one id, in the order AG-UI requires (REASONING_START, REASONING_MESSAGE_START,
#     REASONING_MESSAGE_CONTENT, REASONING_MESSAGE_END, REASONING_END), live first (`vymalo.live` metadata, reported, not asserted: timing) and
#     continued by the log's, all before the reply's TEXT_MESSAGE_START, and the content, put together, is what the model wrote;
#   * the reply is the model's words and holds none of the reasoning, and the thread has one assistant message, not two;
#   * the log (GET /api/threads/{id}/export) has one `agent_reasoning` with the whole reasoning, not cut, and the text occurs nowhere else in it
#     (not in the answer, a step, the status or the final detail);
#   * a reconnect (GET /agui/threads/{id}/connect?mode=run) says the same reasoning once, from the log, before the reply;
#   * a second message in the thread, which the model answers the same way: the requests the model got, both of them, carry no reasoning
#     (it is never sent back: the history holds the first answer's words and nothing of what was thought), and `mock-model` matched every request.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_MODEL_URL   http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   TIMEOUT          120    seconds to wait for a thread to finish
#
# It EMPTIES the request journal of `mock-model` first, so run it on a stack you are not in the middle of another scenario on. Needs curl and jq
# (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
timeout=${TIMEOUT:-120}

text_stream_uri=https://agents.vymalo.com/a2a/extensions/text-stream/v1
thought='The person wants a short answer. I should say what I am doing, and keep it to one line.'
answer='Thinking is on: this is the answer, after the reasoning.'

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "reasoning e2e passed"; else echo "reasoning e2e FAILED"; exit 1; fi
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

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, as a JSON array
  sed -n 's/^data: *//p' "$1" | jq -s '.' 2>/dev/null || echo '[]'
}

# run_message THREAD TEXT OUT: one AG-UI run (a message) on the thread; the SSE response is saved in OUT. Prints the HTTP status.
run_message() {
  _input=$(jq -n --arg thread "$1" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$3" -w '%{http_code}' -X POST "$base/agui/agents/chat" -H "$id_header" \
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

# --- the stack, and the agent the scenario needs ------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" chat "*) ;;
  *) echo "the agent 'chat' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac

echo "== what the agent says it can do, before anyone sends"
chat_caps=$(api GET /agui/agents/chat/capabilities 2>/dev/null || echo '{}')
expect "the capabilities of chat list text-stream/v1 (its card lists it: the orchestrator reads reasoning chunks only then)" \
  "$(printf '%s' "$chat_caps" | jq -r --arg u "$text_stream_uri" '(.custom // {}) | has($u)')" "true"

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $code"; fi

# ===================================================================================================
echo "== a model that thinks, then answers"
thread=$(uuid)
echo "thread $thread (chat)"
code=$(run_message "$thread" "[mock:think] say something short" "$tmp/run1.sse")
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/chat answered HTTP ${code:-none}: $(head -c 300 "$tmp/run1.sse.err") $(head -c 300 "$tmp/run1.sse")"
  finish
fi
sse_events "$tmp/run1.sse" > "$tmp/run1.json"
expect "the run ended with RUN_FINISHED success" "$(outcome_of "$tmp/run1.json")" "success"
expect "the thread ended done" "$(wait_state "$thread")" "done"

# What the stream says about reasoning: the types, in order, of the reasoning events and of the reply's start.
spans=$(jq -r '[.[] | select((.type | startswith("REASONING_")) or (.type == "TEXT_MESSAGE_START" and .role == "assistant")) | .type] | join(" ")' "$tmp/run1.json")
if [ -z "$(jq -r '[.[] | select(.type == "REASONING_START")] | length | select(. > 0)' "$tmp/run1.json")" ]; then
  bad "the stream has no REASONING_START: the agent sent no reasoning (is the coder image at an adam-rs revision with ADR 0020? dev/coder/UPSTREAM)"
  finish
fi
ids=$(jq -r '[.[] | select(.type == "REASONING_START") | .messageId] | unique | length' "$tmp/run1.json")
expect "one reasoning span in the run (one id)" "$ids" "1"
rid=$(jq -r '[.[] | select(.type == "REASONING_START") | .messageId] | first' "$tmp/run1.json")
# The span is the live one then the log's, which continues it: every START once, the content in pieces, the ends once.
expect "the reasoning events are START, MESSAGE_START, MESSAGE_CONTENT (one or more), MESSAGE_END, END, in that order, then the reply" \
  "$(printf '%s' "$spans" | sed -E 's/(REASONING_MESSAGE_CONTENT )+/REASONING_MESSAGE_CONTENT /')" \
  "REASONING_START REASONING_MESSAGE_START REASONING_MESSAGE_CONTENT REASONING_MESSAGE_END REASONING_END TEXT_MESSAGE_START"
# Whether the reasoning arrived live (while the model wrote it) depends on timing, so it is reported and not asserted: the unit and end-to-end
# tests of the orchestrator pin the live frames; this script pins what the stack must always do.
if [ "$(jq -r '[.[] | select(.type == "REASONING_START") | (.metadata["vymalo.live"] != null)] | first' "$tmp/run1.json")" = true ]; then
  ok "the reasoning arrived live (REASONING_START marked vymalo.live, the log's reasoning continued it)"
else
  echo "note the reasoning was said by the log alone (no live piece reached this connection first)"
fi
expect "the content of the span, put together, is what the model wrote" \
  "$(jq -r --arg id "$rid" '[.[] | select(.type == "REASONING_MESSAGE_CONTENT" and .messageId == $id) | .delta] | join("")' "$tmp/run1.json")" "$thought"
expect "the reply is the model's words and none of the reasoning" \
  "$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "assistant") | .messageId] as $ids
    | [.[] | select(.type == "TEXT_MESSAGE_CONTENT" and (.messageId | IN($ids[]))) | .delta] | join("")' "$tmp/run1.json")" "$answer"
expect "one assistant message, not two (the reasoning is not a reply)" \
  "$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "assistant")] | length' "$tmp/run1.json")" "1"

# The log: one agent_reasoning, whole, and the text nowhere else.
api GET "/api/threads/$thread/export" > "$tmp/export1.json" 2>/dev/null || echo '{}' > "$tmp/export1.json"
expect "the log has one agent_reasoning with the whole reasoning, not cut, under the id the screen had" \
  "$(jq -r --arg id "$rid" '[.events[] | select(.kind == "agent_reasoning")] | map([.data.messageId == $id, .data.text, (.data.truncated // false)] | join(" | ")) | join(";")' "$tmp/export1.json")" \
  "true | $thought | false"
expect "the reasoning's words are nowhere else in the log (the answer, the steps and the status hold none of them)" \
  "$(jq -r --arg t "person wants a short" '[.events[] | select(.kind != "agent_reasoning") | tojson | select(contains($t))] | length' "$tmp/export1.json")" "0"

# A reconnect: the log, from the start of the run, says the reasoning once, whole, before the reply.
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null > "$tmp/connect.sse" || true
sse_events "$tmp/connect.sse" > "$tmp/connect.json"
expect "a reconnect says the reasoning once, from the log: five events, then the reply" \
  "$(jq -r '[.[] | select((.type | startswith("REASONING_")) or (.type == "TEXT_MESSAGE_START" and .role == "assistant")) | .type] | join(" ")' "$tmp/connect.json")" \
  "REASONING_START REASONING_MESSAGE_START REASONING_MESSAGE_CONTENT REASONING_MESSAGE_END REASONING_END TEXT_MESSAGE_START"
expect "the replayed reasoning is the same text" \
  "$(jq -r '[.[] | select(.type == "REASONING_MESSAGE_CONTENT") | .delta] | join("")' "$tmp/connect.json")" "$thought"

# ===================================================================================================
echo "== a second message: the reasoning is never sent back to the model"
code=$(run_message "$thread" "[mock:think] and once more" "$tmp/run2.sse")
if [ "$code" != 200 ]; then bad "the second POST answered HTTP ${code:-none}: $(head -c 300 "$tmp/run2.sse")"; fi
expect "the thread ended done again" "$(wait_state "$thread")" "done"
sse_events "$tmp/run2.sse" > "$tmp/run2.json"
expect "the second run has its own reasoning, another id" \
  "$(jq -r --arg id "$rid" '[.[] | select(.type == "REASONING_START" and .messageId != $id)] | length | . > 0' "$tmp/run2.json")" "true"

curl -s --max-time 30 "$model/__admin/requests" |
  jq -c '[.requests | reverse | .[].request.body | fromjson? | select(.model == "mock-persona")]' > "$tmp/requests.json" 2>/dev/null || echo '[]' > "$tmp/requests.json"
expect "the model got two requests, one for each message" "$(jq -r 'length' "$tmp/requests.json")" "2"
expect "the second request has the first answer's words in its history" \
  "$(jq -r --arg a "$answer" '.[1] | [.messages[] | select(.role == "assistant") | (.content // "")] | any(contains($a))' "$tmp/requests.json" 2>/dev/null || echo false)" "true"
expect "no request carries the reasoning (not its words, not a reasoning member on a message)" \
  "$(jq -r '[.[] | tojson | select(contains("person wants a short") or contains("reasoning_content") or contains("\"reasoning\""))] | length' "$tmp/requests.json")" "0"
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
expect "mock-model matched every request" "$unmatched" "0"

finish
