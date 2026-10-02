#!/usr/bin/env sh
# System-level test of the split roles (ADR 0015): a control plane and two workers over one
# Postgres. The worker that holds a delegation is killed with SIGKILL, and the other one finishes it.
#
#   ORCHESTRATOR_ROLE=control-plane docker compose --profile app --profile split up -d --build --wait
#   dev/split-e2e.sh
#
# The `orchestrator` service must be the control plane (ORCHESTRATOR_ROLE=control-plane, see
# compose.yaml): as `all` it would deliver the task itself, and there would be no worker to kill.
#
# The script speaks AG-UI, as the UI does (docs/api/agui.md): one POST /agui/agents/{agentId} runs
# the thread (its response streams while the task runs, so the script reads it from the background),
# the thread state comes from the resource API, and the events are the thread's AG-UI frames. The
# legacy chat API routes were removed on 2026-09-30. It prints one ok or FAIL line per check; it
# exits 1 if any failed:
#   * GET /metrics answers on the control plane and on both workers, with the outbox gauge;
#   * a thread for the mock agent with the keyword `slow` (an 8 s answer, dev/README.md) reaches
#     `working`, and its delegate row is held by orchestrator-worker-1 or -2 (`lease_owner`);
#   * that worker is killed (`docker compose kill -s SIGKILL`) mid-task, and the thread still ends
#     `done` on the other worker: it takes the row over when the 5 s lease lapses;
#   * the delegate row was claimed twice (attempts = 2) and ended `delivered`;
#   * the thread's frames (GET /agui/threads/{id}/connect?mode=run) hold exactly one run of the agent that
#     ends RUN_FINISHED (success) and no RUN_ERROR: the task finished once, not once per worker. A run that
#     holds nothing but a state snapshot is not the agent's: the title the orchestrator's model writes after
#     the reply (ADR 0005, `thread_titled`) is one, a run of its own after the agent's, and may or may not be
#     there yet when the frames are read;
#   * the control plane's /metrics then reports no due and no leased row;
#   * live text across the processes (ADR 0027): a thread with the keyword `stream` (a reply in six
#     chunks over about 6 s, dev/README.md) is held by the surviving worker, and a viewer connected
#     to the control plane reads the reply grow: at least three live deltas of one message before the
#     log's message completes it, the deltas joined by offset are the final text, the message starts
#     once, a viewer that reconnects mid-stream with `Last-Event-ID` reads it once too, and the
#     export holds one `agent_message` with that id and none that is not final.
# The killed worker is started again when the script ends, whatever the result.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL          http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL        dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   AGENT_ID          mock-coder, the mock A2A agent whose `slow` keyword is used
#   COMPOSE_PROFILES  app,split unless set, so `docker compose` can address the worker services
#   TIMEOUT           120    seconds to wait for each step
#
# Needs curl, jq and docker compose (and /proc or uuidgen for a UUID; the metrics are read from inside the network, through the `edge`
# container, and the database through the `postgres` container). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
agent_id=${AGENT_ID:-mock-coder}
timeout=${TIMEOUT:-120}
export COMPOSE_PROFILES="${COMPOSE_PROFILES:-app,split}"

# `docker compose` looks for compose.yaml in the working directory and its parents.
cd "$(dirname "$0")/.."

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

tmp=$(mktemp -d)
killed=
run_pid=
cleanup() {
  if [ -n "$run_pid" ]; then kill "$run_pid" 2>/dev/null || true; fi
  rm -rf "$tmp"
  if [ -n "$killed" ]; then
    echo "starting $killed again"
    docker compose up -d --no-build --wait "$killed" >/dev/null 2>&1 || echo "could not start $killed again"
  fi
}
trap cleanup EXIT

finish() {
  if [ "$fail" -eq 0 ]; then echo "split e2e passed"; else echo "split e2e FAILED"; exit 1; fi
}

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

metrics() { # metrics SERVICE: the /metrics text of a service, read from inside the compose network
  docker compose exec -T edge wget -qO- "http://$1:8080/metrics"
}

gauge() { # gauge SERVICE STATE: the value of orch_outbox_rows{state=STATE}, empty if unreadable
  metrics "$1" 2>/dev/null | sed -n "s/^orch_outbox_rows{state=\"$2\"} //p"
}

sql() { # sql QUERY: one unaligned, tuples-only result on stdout
  docker compose exec -T postgres psql -U postgres -d orch -Atc "$1"
}

thread_state() { # thread_state ID
  api GET "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' || true
}

# --- every process serves /metrics ----------------------------------------------------------------
# `up --wait` returns once the containers run, which is before a process has migrated and bound its
# port (these services have no healthcheck: the image has no shell), so each one gets some time.
for svc in orchestrator orchestrator-worker-1 orchestrator-worker-2; do
  deadline=$(( $(date +%s) + 60 ))
  while :; do
    out=$(metrics "$svc" 2>"$tmp/err" || true)
    if printf '%s\n' "$out" | grep -q '^# TYPE orch_outbox_rows gauge$'; then
      ok "$svc serves /metrics"
      break
    fi
    if [ "$(date +%s)" -ge "$deadline" ]; then
      bad "$svc does not serve /metrics: $(head -c 300 "$tmp/err") $(printf '%s' "$out" | head -c 200)"
      break
    fi
    sleep 2
  done
done
if [ "$fail" -ne 0 ]; then
  echo "is the split profile up? docker compose --profile app --profile split ps"
  finish
fi

# --- a slow task ------------------------------------------------------------------------------------
# The keyword `slow` makes the mock agent answer over 8 s (dev/README.md), so the task is still
# running when its worker dies.
# The consumer mints the thread id (a UUID); the first run creates the thread. The response streams
# until the run ends, which is after the worker below is killed, so the request runs in the
# background and its answer (the HTTP status, then the frames) is read from files.
thread=$(uuid)
echo "thread $thread"
case $thread in
  *[!0-9a-f-]* | '') bad "the thread id '$thread' is not a UUID"; finish ;;
esac
input=$(jq -n --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: "a slow task for the split roles"}], forwardedProps: {}}')
curl -sS -N --max-time $(( timeout * 3 )) -o "$tmp/run.sse" -w '%{http_code}' -X POST \
  "$base/agui/agents/$agent_id" -H "$id_header" \
  -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" \
  > "$tmp/run.code" 2>"$tmp/err" &
run_pid=$!

deadline=$(( $(date +%s) + timeout ))
state=
while :; do
  state=$(thread_state "$thread")
  case $state in working | done | failed | blocked | cancelled) break ;; esac
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 1
done
if [ "$state" = working ]; then
  ok "the thread is working"
else
  bad "the thread is '${state:-unknown}', want working (a worker must be delivering; done already means the task was not slow; run answered HTTP $(cat "$tmp/run.code" 2>/dev/null || true): $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse" 2>/dev/null))"
  finish
fi

# --- who holds it ----------------------------------------------------------------------------------------
owner=$(sql "SELECT lease_owner FROM outbox WHERE thread_id = '$thread' AND kind = 'delegate' AND status = 'inflight'" 2>"$tmp/err" || true)
case $owner in
  orchestrator-worker-1 | orchestrator-worker-2)
    ok "the delegation is held by $owner"
    ;;
  *)
    bad "the delegation is held by '${owner:-nobody}' ($(head -c 200 "$tmp/err")), want orchestrator-worker-1 or -2: was the stack started with ORCHESTRATOR_ROLE=control-plane?"
    finish
    ;;
esac

# --- kill it ------------------------------------------------------------------------------------------------
killed=$owner
if docker compose kill -s SIGKILL "$owner" >/dev/null 2>"$tmp/err"; then
  ok "$owner killed with SIGKILL"
else
  bad "cannot kill $owner: $(head -c 300 "$tmp/err")"
  finish
fi

# --- the other worker finishes the task -------------------------------------------------------------------------
deadline=$(( $(date +%s) + timeout ))
state=
while :; do
  state=$(thread_state "$thread")
  case $state in done | failed | blocked | cancelled) break ;; esac
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 1
done
# The thread's AG-UI frames as one JSON array: the viewer replay, which closes after the run.
events=$tmp/events.json
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
  "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
  echo '[]' > "$events"
if [ "$state" = "done" ]; then
  ok "the thread ended done after $owner died"
else
  bad "the thread did not end done (state: '${state:-unknown}' after at most ${timeout}s)"
  jq -r '.[] | "     \(.type) \(.snapshot.thread.state // .content.status // .outcome.type // .message // "")"' "$events" | head -n 20
fi

# --- the outbox row ---------------------------------------------------------------------------------------------
# The thread is done as soon as the last update is committed; the row is closed a moment later.
deadline=$(( $(date +%s) + 30 ))
row=
while :; do
  row=$(sql "SELECT concat(attempts, '/', status) FROM outbox WHERE thread_id = '$thread' AND kind = 'delegate'" 2>/dev/null || true)
  case $row in */delivered) break ;; esac
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 1
done
if [ "$row" = 2/delivered ]; then
  ok "the delegate row was claimed twice and ended delivered"
else
  bad "the delegate row is '${row:-missing}' (attempts/status), want 2/delivered"
fi

# --- the events -----------------------------------------------------------------------------------------------------
# The frames are cut into runs at each RUN_STARTED. A run whose frames are all RUN_STARTED, STATE_SNAPSHOT and
# RUN_FINISHED holds no work of the agent (a title or a rename outside a run is one: docs/api/agui.md "Titles"),
# so only the other runs are the agent's. Any RUN_ERROR frame counts, in whatever run.
# shellcheck disable=SC2016 # a jq program: its $ are jq's
agent_runs='def runs: reduce .[] as $f ([]; if $f.type == "RUN_STARTED" then . + [[$f]] elif length == 0 then . else .[length - 1] += [$f] end);
  def state_only: all(.[]; .type == "RUN_STARTED" or .type == "STATE_SNAPSHOT" or .type == "RUN_FINISHED");
  [runs[] | select(state_only | not)]'
dones=$(jq -r "$agent_runs"' | [.[] | select(.[-1].type == "RUN_FINISHED" and (.[-1].outcome.type // "success") == "success")] | length' "$events" 2>/dev/null || echo '?')
agent_total=$(jq -r "$agent_runs"' | length' "$events" 2>/dev/null || echo '?')
errors=$(jq -r '[.[] | select(.type == "RUN_ERROR")] | length' "$events" 2>/dev/null || echo '?')
if [ "$dones" = 1 ] && [ "$agent_total" = 1 ] && [ "$errors" = 0 ]; then
  ok "exactly one run of the agent, ended RUN_FINISHED (success), and no RUN_ERROR"
else
  bad "$dones of $agent_total runs of the agent ended RUN_FINISHED (success) and $errors RUN_ERROR frames, want exactly 1 of 1 and 0 ($(jq -c '[.[] | .type]' "$events" 2>/dev/null))"
fi

# --- the queue is empty again ------------------------------------------------------------------------------------------
deadline=$(( $(date +%s) + 30 ))
due=
leased=
while :; do
  due=$(gauge orchestrator due)
  leased=$(gauge orchestrator leased)
  if [ "$due" = 0 ] && [ "$leased" = 0 ]; then break; fi
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 1
done
if [ "$due" = 0 ] && [ "$leased" = 0 ]; then
  ok "the control plane's /metrics reports no due and no leased row"
else
  bad "the control plane's /metrics reports due='${due:-?}' leased='${leased:-?}', want 0 and 0"
fi

# --- live text across the processes -------------------------------------------------------------------------------------
# The keyword `stream` makes the mock agent send a reply as six chunks over about 6 s. The worker that
# holds the agent's stream publishes each piece on Postgres (`NOTIFY orch_live`, ADR 0027); the control
# plane, another process, shows the words growing on a viewer's connect stream. The mock's text is ASCII,
# so the offsets (UTF-16 code units on the wire) are also jq's string positions.
stream_thread=$(uuid)
echo "thread $stream_thread (live text)"
input=$(jq -n --arg thread "$stream_thread" --arg run "$(uuid)" --arg msg "$(uuid)" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: "stream a reply across the processes"}], forwardedProps: {}}')
curl -sS -N --max-time $(( timeout * 2 )) -o "$tmp/stream-run.sse" -X POST \
  "$base/agui/agents/$agent_id" -H "$id_header" \
  -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" \
  >/dev/null 2>"$tmp/err" &
run_pid=$!
# The viewer connects as soon as the thread exists, which is before the reply starts (the mock dribbles it).
deadline=$(( $(date +%s) + 30 ))
until api GET "/api/threads/$stream_thread" >/dev/null 2>&1; do
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 0.2
done
# A second viewer reconnects mid-stream with the id of a log event it holds (2: the agent's `working`),
# about 2 s into the reply: it is told the text so far by the sender's refresh, then the log's message.
(
  sleep 2
  curl -sS --max-time "$timeout" -H "$id_header" -H 'accept: text/event-stream' \
    -H 'Last-Event-ID: 2' "$base/agui/threads/$stream_thread/connect?mode=run" 2>/dev/null |
    sed -n 's/^data: *//p' | jq -s '.' > "$tmp/late.json" 2>/dev/null || echo '[]' > "$tmp/late.json"
) &
late_pid=$!
live=$tmp/live.json
curl -sS --max-time "$timeout" -H "$id_header" -H 'accept: text/event-stream' \
  "$base/agui/threads/$stream_thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$live" 2>/dev/null ||
  echo '[]' > "$live"
reply=$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .metadata["vymalo.live"] != null) | .messageId] | first // empty' "$live")
if [ -n "$reply" ]; then
  ok "the control plane's connect stream opened a live message ($reply)"
else
  bad "no live message on the control plane's connect stream ($(jq -c '[.[] | .type]' "$live" 2>/dev/null | head -c 300))"
fi
grown=$(jq -r --arg id "$reply" '[.[] | select(.type == "TEXT_MESSAGE_CONTENT" and .messageId == $id and .metadata["vymalo.live"] != null and (.metadata["vymalo.live"].final | not))] | length' "$live" 2>/dev/null || echo 0)
if [ "${grown:-0}" -ge 3 ]; then
  ok "the reply grew: $grown live deltas before the log's message completed it"
else
  bad "$grown live deltas for the reply, want at least 3"
fi
read_text=$(jq -r --arg id "$reply" 'reduce (.[] | select(.type == "TEXT_MESSAGE_CONTENT" and .messageId == $id)) as $c ("";
  (if $c.metadata["vymalo.live"].offset != null then .[0:$c.metadata["vymalo.live"].offset] else . end) + $c.delta)' "$live" 2>/dev/null || true)
want_text='Streaming a reply, word by word, so the chat can show it grow.'
if [ "$read_text" = "$want_text" ]; then
  ok "the deltas joined by offset are the final text"
else
  bad "the viewer reads '$read_text', want '$want_text'"
fi
starts=$(jq -r --arg id "$reply" '[.[] | select(.type == "TEXT_MESSAGE_START" and .messageId == $id)] | length' "$live" 2>/dev/null || echo '?')
ends=$(jq -r --arg id "$reply" '[.[] | select(.type == "TEXT_MESSAGE_END" and .messageId == $id)] | length' "$live" 2>/dev/null || echo '?')
if [ "$starts" = 1 ] && [ "$ends" = 1 ]; then
  ok "the reply starts once and ends once on the connect stream"
else
  bad "the reply starts $starts time(s) and ends $ends time(s), want once each"
fi
wait "$late_pid" 2>/dev/null || true
late_starts=$(jq -r --arg id "$reply" '[.[] | select(.type == "TEXT_MESSAGE_START" and .messageId == $id)] | length' "$tmp/late.json" 2>/dev/null || echo '?')
late_text=$(jq -r --arg id "$reply" 'reduce (.[] | select(.type == "TEXT_MESSAGE_CONTENT" and .messageId == $id)) as $c ("";
  (if $c.metadata["vymalo.live"].offset != null then .[0:$c.metadata["vymalo.live"].offset] else . end) + $c.delta)' "$tmp/late.json" 2>/dev/null || true)
if [ "$late_starts" = 1 ] && [ "$late_text" = "$want_text" ]; then
  ok "a viewer that reconnected mid-stream (Last-Event-ID: 2) reads the reply once, whole"
else
  bad "the reconnected viewer starts the reply $late_starts time(s) and reads '$late_text', want once and '$want_text'"
fi
# The export is the log: one final message, no partial.
export_json=$tmp/stream-export.json
if api GET "/api/threads/$stream_thread/export" > "$export_json" 2>"$tmp/err"; then
  finals=$(jq -r --arg id "$reply" '[.events[] | select(.kind == "agent_message" and .data.messageId == $id and .data.final == true)] | length' "$export_json")
  partials=$(jq -r '[.events[] | select(.kind == "agent_message" and .data.final == false)] | length' "$export_json")
  if [ "$finals" = 1 ] && [ "$partials" = 0 ]; then
    ok "the export holds one agent_message with the reply's id and none that is not final"
  else
    bad "the export holds $finals final agent_message(s) with that id and $partials partial one(s), want 1 and 0"
  fi
else
  bad "cannot export the thread: $(head -c 300 "$tmp/err")"
fi
replay=$tmp/replay.json
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
  "$base/agui/threads/$stream_thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$replay" 2>/dev/null ||
  echo '[]' > "$replay"
replay_live=$(jq -r '[.[] | select(.metadata["vymalo.live"] != null)] | length' "$replay" 2>/dev/null || echo '?')
replay_starts=$(jq -r --arg id "$reply" '[.[] | select(.type == "TEXT_MESSAGE_START" and .messageId == $id)] | length' "$replay" 2>/dev/null || echo '?')
if [ "$replay_live" = 0 ] && [ "$replay_starts" = 1 ]; then
  ok "a connection opened after the reply is in the log reads the plain message, with no live frame"
else
  bad "the replay holds $replay_live live frame(s) and starts the reply $replay_starts time(s), want 0 and 1"
fi
if [ -n "$run_pid" ]; then kill "$run_pid" 2>/dev/null || true; run_pid=; fi

finish
