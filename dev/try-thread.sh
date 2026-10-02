#!/usr/bin/env sh
# Drive one chat thread against a running orchestrator, over AG-UI, and print its events.
#
#   dev/try-thread.sh "add a health endpoint"                 # mock-coder (see AGENT_ID below)
#   AGENT_ID=mock-coder-releases RELEASE=staging dev/try-thread.sh "do it"
#   dev/try-thread.sh "ask which branch"                      # ends `blocked` ...
#   THREAD_ID=<id> dev/try-thread.sh "main"                   # ... answer it (a follow-up message)
#
# It speaks what the web speaks (docs/api/agui.md): a run is one POST /agui/agents/{agentId} with a
# thread id the script mints (a UUID), whose response streams until the run ends; the thread's state
# is read from the resource API (GET /api/threads/{id}); the events printed are the thread's AG-UI
# frames, replayed by GET /agui/threads/{id}/connect?mode=run. The legacy chat API routes
# (POST /api/threads, .../messages, .../events, .../stream) were removed on 2026-09-30.
#
# Environment (all optional):
#   BASE_URL    where the API is served     (default http://127.0.0.1:8080, the compose `edge`)
#   AGENT_ID    target agent id             (default mock-coder, NOT the default agent of dev/agents.yaml,
#               which is the real `coder`: use dev/coder-e2e.sh for that one; see dev/agents.yaml)
#   RELEASE     channel or revision         (only for mock-coder-releases)
#   THREAD_ID   send TEXT as a follow-up to this thread instead of creating one
#   AUTH_EMAIL  the user (default dev@example.com): a token of the mock issuer (dev/auth-header.sh), which the
#               compose `edge` wants. Talking to `cargo run` directly with `auth.mode: proxy_header` (or
#               AUTH_DEV_USER), use AUTH_MODE=proxy-header: it sends X-Auth-Request-Email, which identifies you.
#   TIMEOUT     seconds to wait for the thread to stop moving (default 60)
#
# Exit status: 0 when the thread ends `done` or `blocked`, 1 on `failed`/`cancelled` or a timeout.
# Needs: curl, jq (and /proc or uuidgen for a UUID).
set -eu

BASE_URL=${BASE_URL:-http://127.0.0.1:8080}
AGENT_ID=${AGENT_ID:-mock-coder}
AUTH_EMAIL=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$AUTH_EMAIL")
TIMEOUT=${TIMEOUT:-60}
[ $# -ge 1 ] || { echo "usage: $0 TEXT..." >&2; exit 2; }
TEXT=$*

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api PATH: GET on the resource API, non-zero when the status is not 2xx
  curl --fail-with-body -sS -H "$id_header" "$BASE_URL$1"
}

ID=${THREAD_ID:-$(uuid)}
# The release channel or revision (ADR 0008) travels in forwardedProps, under the extension's URI.
INPUT=$(jq -n --arg thread "$ID" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$TEXT" \
  --arg release "${RELEASE:-}" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}],
    forwardedProps: (if $release == "" then {} else
      {"https://agents.vymalo.com/a2a/extensions/release-channels/v1": {release: $release}} end)
  }')
echo "thread $ID" >&2

# The response streams until the run ends (a finished, blocked or failed run), or until TIMEOUT.
# Closing it early would not cancel the run; the state loop below is what decides.
deadline=$(( $(date +%s) + TIMEOUT ))
code=$(curl -sS -N --max-time "$TIMEOUT" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
  "$BASE_URL/agui/agents/$AGENT_ID" -H "$id_header" \
  -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$INPUT" || true)
if [ "$code" != 200 ]; then
  echo "POST /agui/agents/$AGENT_ID answered HTTP ${code:-none}: $(head -c 400 "$tmp/run.sse" 2>/dev/null)" >&2
  exit 1
fi

while :; do
  state=$(api "/api/threads/$ID" | jq -r .state)
  case $state in done | blocked | failed | cancelled) break ;; esac
  [ "$(date +%s)" -lt "$deadline" ] || { echo "timed out in state $state" >&2; exit 1; }
  sleep 0.5
done

# The thread's frames, one per line: the whole conversation, every run. `mode=run` ends the stream
# once the replay is done and no run is open.
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
  "$BASE_URL/agui/threads/$ID/connect?mode=run" | sed -n 's/^data: *//p' | jq -rs '
  to_entries[] | .key as $n | .value
  | ((.metadata // {})["vymalo.actor"].revision // "") as $rev
  | (if $rev == "" then "" else "[\($rev)] " end) as $r
  | "\($n + 1)\t\(.type)\t" + (
      if .type == "TEXT_MESSAGE_START" then "\($r)\(.role)"
      elif .type == "TEXT_MESSAGE_CONTENT" then .delta
      elif .type == "ACTIVITY_SNAPSHOT" then
        "\($r)\(.activityType) " + (
          if .activityType == "vymalo.status" then "\(.content.status): \(.content.detail // "")"
          elif .activityType == "vymalo.artifact" then "\(.content.name) \(.content.uri // .content.text // "")"
          else (.content | tostring) end)
      elif .type == "STATE_SNAPSHOT" then .snapshot.thread.state
      elif .type == "RUN_STARTED" then .runId
      elif .type == "RUN_FINISHED" then (.outcome.type // "success")
      elif .type == "RUN_ERROR" then "\(.code // ""): \(.message)"
      elif .type == "SUBAGENT_STARTED" then "\($r)\(.name)"
      else "" end)'
echo "state: $state" >&2
case $state in done | blocked) exit 0 ;; *) exit 1 ;; esac
