#!/usr/bin/env sh
# Drive one chat thread against a running orchestrator and print its event log.
#
#   dev/try-thread.sh "add a health endpoint"                 # default agent: mock-coder
#   AGENT_ID=mock-coder-releases RELEASE=staging dev/try-thread.sh "do it"
#   dev/try-thread.sh "ask which branch"                      # ends `blocked` ...
#   THREAD_ID=<id> dev/try-thread.sh "main"                   # ... answer it (a follow-up message)
#
# Environment (all optional):
#   BASE_URL    where the API is served     (default http://127.0.0.1:8080, the compose `edge`)
#   AGENT_ID    target agent id             (default mock-coder; see dev/agents.yaml)
#   RELEASE     channel or revision         (only for mock-coder-releases)
#   THREAD_ID   post TEXT as a follow-up to this thread instead of creating one
#   AUTH_EMAIL  X-Auth-Request-Email to send (default dev@example.com). Behind the compose
#               `edge` the proxy sets it anyway; when talking to `cargo run` directly it is
#               what identifies you.
#   TIMEOUT     seconds to wait for the thread to stop moving (default 60)
#
# Exit status: 0 when the thread ends `done` or `blocked`, 1 on `failed`/`cancelled` or a timeout.
# Needs: curl, jq.
set -eu

BASE_URL=${BASE_URL:-http://127.0.0.1:8080}
AGENT_ID=${AGENT_ID:-mock-coder}
AUTH_EMAIL=${AUTH_EMAIL:-dev@example.com}
TIMEOUT=${TIMEOUT:-60}
[ $# -ge 1 ] || { echo "usage: $0 TEXT..." >&2; exit 2; }
TEXT=$*

api() { # api METHOD PATH [JSON]
  if [ $# -ge 3 ]; then
    curl --fail-with-body -sS -X "$1" "$BASE_URL$2" -H "X-Auth-Request-Email: $AUTH_EMAIL" \
      -H 'content-type: application/json' -d "$3"
  else
    curl --fail-with-body -sS -X "$1" "$BASE_URL$2" -H "X-Auth-Request-Email: $AUTH_EMAIL"
  fi
}

if [ -n "${THREAD_ID:-}" ]; then
  api POST "/api/threads/$THREAD_ID/messages" "$(jq -n --arg t "$TEXT" '{text: $t}')" >/dev/null || exit 1
  ID=$THREAD_ID # posting runs the transition inside the request: the thread is `queued` at once
else
  BODY=$(jq -n --arg t "$TEXT" --arg a "$AGENT_ID" --arg r "${RELEASE:-}" \
    '{text: $t, target: ({agentId: $a} + (if $r == "" then {} else {release: $r} end))}')
  CREATED=$(api POST /api/threads "$BODY") || { echo "$CREATED" >&2; exit 1; }
  ID=$(printf '%s' "$CREATED" | jq -r .id)
fi
echo "thread $ID" >&2

deadline=$(( $(date +%s) + TIMEOUT ))
while :; do
  state=$(api GET "/api/threads/$ID" | jq -r .state)
  case $state in done | blocked | failed | cancelled) break ;; esac
  [ "$(date +%s)" -lt "$deadline" ] || { echo "timed out in state $state" >&2; exit 1; }
  sleep 0.5
done

api GET "/api/threads/$ID/events" | jq -r '.[] |
  "\(.seq)\t\(.kind)\t" + (if .actor.revision then "[\(.actor.revision)] " else "" end) + (
    if .kind == "thread_state" then .data.state
    elif .kind == "agent_status" then "\(.data.status): \(.data.detail // "")"
    elif .kind == "artifact" then "\(.data.name) \(.data.uri // .data.text // "")"
    else (.data.text // .data.message // "") end)'
echo "state: $state" >&2
case $state in done | blocked) exit 0 ;; *) exit 1 ;; esac
