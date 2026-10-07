#!/usr/bin/env sh
# System-level test of the default agent's name and greeting (adam-rs#55): "hi" in the chat gets a
# greeting that says who the agent is, not a request for a task.
#
#   dev/greeting-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the UI does (docs/api/agui.md): it runs a thread for the default agent (the
# first entry of dev/agents.yaml) with one POST /agui/agents/{agentId} whose message is `hi`, waits for the
# thread to stop, and reads the agent's words from the thread's frames
# (GET /agui/threads/{id}/connect?mode=run). The coder asks the person what to do, so the thread ends
# `blocked` (an A2A `input_required`), and its question is the greeting.
#
# The model is the `mock-coder` script (dev/coder/wiremock/mock-openai, see dev/coder/UPSTREAM). Its
# greeting is built from the first two lines of the coder's instructions, `Your name is {{display_name}}.`
# and `In one sentence: <summary>.`, as the coder rendered them into its system prompt. So the answer
# says what the folder mounted at /etc/adam/agent says: the script reads the name and the summary from
# that folder (dev/coder/agent, see AGENT_DIR) and expects them back.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the default agent of GET /api/agents is `adam`;
#   * the run stream ends with RUN_FINISHED whose outcome is an interrupt (the agent waits for the person),
#     and the thread ends `blocked`, not `failed` and not `done`;
#   * the agent's words say "I'm <name>" and the one-sentence summary, and ask what it can help with (since adam-rs
#     4363924, adam-rs ADR 0021: a greeting asks an open question, no longer which repository): no "give me a task" and no
#     tool name; no artifact (no tool ran);
#   * mock-openai saw a mock-coder request whose system prompt holds `Your name is <name>.` and the
#     `In one sentence:` line of the folder, and matched every request.
# Exit status 0 when every check passed.
#
# dev/agent-folder-e2e.sh runs this script a second time with AGENT_DIR pointing at a copy of the folder that
# has another name, after it has restarted the coder on that copy.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}
#   AGENT_DIR        dev/coder/agent   the folder the coder is running on; the name and the summary are read from it
#   TIMEOUT          120    seconds to wait for the thread to stop
#
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
openai=${MOCK_OPENAI_URL:-http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}}
openai=${openai%/}
timeout=${TIMEOUT:-120}

root=$(cd "$(dirname "$0")/.." && pwd)
agent_dir=${AGENT_DIR:-$root/dev/coder/agent}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "greeting e2e passed"; else echo "greeting e2e FAILED"; exit 1; fi
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

# --- the persona the folder gives -------------------------------------------------------------------
# `display_name: <name>` under `vars` in the frontmatter, and the line `In one sentence: <summary>.` that opens
# the body. The mock ends the summary at its first period, so it must not hold one (dev/coder/agent upstream).
instructions=$agent_dir/instructions.md
if [ ! -f "$instructions" ]; then
  echo "FAIL $instructions does not exist (AGENT_DIR is the folder that holds instructions.md)"
  exit 1
fi
name=$(sed -n 's/^[[:space:]]*display_name:[[:space:]]*//p' "$instructions" | head -n 1)
summary=$(sed -n 's/^In one sentence: \([^.]*\)\..*$/\1/p' "$instructions" | head -n 1)
if [ -z "$name" ] || [ -z "$summary" ]; then
  echo "FAIL $instructions has no 'display_name:' var or no 'In one sentence: <summary>.' line"
  exit 1
fi
echo "persona of $agent_dir: $name, \"$summary\""

# --- the default agent -------------------------------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  agent_id=$(printf '%s' "$agents" | jq -r '.[0].id // empty')
  if [ "$agent_id" = adam ]; then
    ok "the default agent (first of /api/agents) is adam"
  else
    bad "the default agent is '${agent_id:-none}', want adam (agents: $(printf '%s' "$agents" | jq -c '[.[].id]'))"
    finish
  fi
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi

# --- say hi --------------------------------------------------------------------------------------------
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$openai/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $openai"; else bad "journal reset: $openai answered HTTP $code"; fi

thread=$(uuid)
input=$(jq -n --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "hi" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
echo "thread $thread"
deadline=$(( $(date +%s) + timeout ))
code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
  "$base/agui/agents/$agent_id" -H "$id_header" \
  -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" 2>"$tmp/err" || true)
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/$agent_id answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
# The terminal event of the run: an agent that waits for the person closes it as an interrupt.
outcome=$(sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
  | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true)
if [ "$outcome" = interrupt ]; then
  ok "the run stream ended with RUN_FINISHED (interrupt): the agent waits for the person"
else
  bad "the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (interrupt)"
fi

# `blocked` ends the wait: nothing here answers the agent.
state=
while :; do
  state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
  case $state in done | blocked | failed | cancelled) break ;; esac
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 2
done
events=$tmp/events.json
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
  "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
  echo '[]' > "$events"
if [ "$state" = blocked ]; then
  ok "the thread ended blocked (the agent asked the person something)"
else
  bad "the thread ended '${state:-unknown}' (after at most ${timeout}s), want blocked: a greeting is not a task"
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$events" | head -n 20
fi

# --- what the agent said --------------------------------------------------------------------------------
# The words of every assistant message of the thread, in order: the greeting is the coder's `input_required`.
said=$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "assistant") | .messageId] as $ids
  | [.[] | select(.type == "TEXT_MESSAGE_CONTENT" and (.messageId | IN($ids[]))) | .delta] | join(" ")' "$events" 2>/dev/null || true)
echo "the agent said: ${said:-<nothing>}"
case $said in
  *"I'm $name"*) ok "it says its name: I'm $name" ;;
  *) bad "the answer does not say \"I'm $name\"" ;;
esac
case $said in
  *"$summary"*) ok "it says what it does: $summary" ;;
  *) bad "the answer does not say what it does (\"$summary\")" ;;
esac
case $said in
  *"What can I help with"*) ok "it asks what it can help with" ;;
  *) bad "the answer does not ask what it can help with" ;;
esac
artifacts=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact") | .content.name] | join(" ")' "$events" 2>/dev/null || true)
if [ -z "$artifacts" ]; then ok "no tool ran: no artifact"; else bad "a greeting produced artifacts: $artifacts"; fi

# --- mock-openai's journal ---------------------------------------------------------------------------
# The first request of the coder's model: messages[0] is the system prompt, the folder rendered with its vars.
prompt=$(curl -s --max-time 30 "$openai/__admin/requests" |
  jq -r '[.requests[].request.body | fromjson? | select(.model == "mock-coder") | .messages[0].content] | first // empty' 2>/dev/null || true)
case $prompt in
  *"Your name is $name."*) ok "the coder's system prompt says: Your name is $name." ;;
  *) bad "the coder's system prompt does not hold \"Your name is $name.\" (is the folder mounted at /etc/adam/agent the one in AGENT_DIR?)" ;;
esac
case $prompt in
  *"In one sentence: $summary"*) ok "the coder's system prompt holds the one-sentence line of the folder" ;;
  *) bad "the coder's system prompt does not hold \"In one sentence: $summary\"" ;;
esac
unmatched=$(curl -s --max-time 30 "$openai/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then
  ok "mock-openai matched every request"
else
  bad "mock-openai saw $unmatched unmatched requests (see $openai/__admin/requests/unmatched)"
fi

finish
