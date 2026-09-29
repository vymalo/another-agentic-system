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
# deprecated chat API routes are not used: they are served only with ORCH_SURFACES=agui,chat-api,
# and compose does not set that. It prints one ok or FAIL line per check; it exits 1 if any failed:
#   * GET /metrics answers on the control plane and on both workers, with the outbox gauge;
#   * a thread for the mock agent with the keyword `slow` (an 8 s answer, dev/README.md) reaches
#     `working`, and its delegate row is held by orchestrator-worker-1 or -2 (`lease_owner`);
#   * that worker is killed (`docker compose kill -s SIGKILL`) mid-task, and the thread still ends
#     `done` on the other worker: it takes the row over when the 5 s lease lapses;
#   * the delegate row was claimed twice (attempts = 2) and ended `delivered`;
#   * the thread's frames (GET /agui/threads/{id}/connect?mode=run) hold exactly one RUN_FINISHED
#     (success) and no RUN_ERROR: the task finished once, not once per worker;
#   * the control plane's /metrics then reports no due and no leased row.
# The killed worker is started again when the script ends, whatever the result.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL          http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`, which injects the identity
#   AUTH_EMAIL        dev@example.com, sent as X-Auth-Request-Email (the edge replaces it)
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
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "X-Auth-Request-Email: $email"
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
  "$base/agui/agents/$agent_id" -H "X-Auth-Request-Email: $email" \
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
curl -sS --max-time 60 -H "X-Auth-Request-Email: $email" -H 'accept: text/event-stream' \
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
dones=$(jq -r '[.[] | select(.type == "RUN_FINISHED" and (.outcome.type // "success") == "success")] | length' "$events" 2>/dev/null || echo '?')
errors=$(jq -r '[.[] | select(.type == "RUN_ERROR")] | length' "$events" 2>/dev/null || echo '?')
if [ "$dones" = 1 ] && [ "$errors" = 0 ]; then
  ok "exactly one RUN_FINISHED (success) and no RUN_ERROR"
else
  bad "$dones RUN_FINISHED (success) and $errors RUN_ERROR frames, want exactly 1 and 0 ($(jq -c '[.[] | .type]' "$events" 2>/dev/null))"
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

finish
