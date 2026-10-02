#!/usr/bin/env sh
# System-level test of the orchestrator's second utility model task (ADR 0035): when a job ends the thread is given a
# description, a sentence or two on what it is about now, by a model of its own at an endpoint of its own; a person's
# edit of it is final, an empty one included; a fork keeps it; and the public configuration says whether the web shows it.
#
#   dev/description-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; the `chat` agent runs from the same
# image):
#
#   docker compose --profile app up -d --build --wait
#
# The model is `mock-description` on the WireMock `mock-model` (dev/wiremock/model/mappings/description.json): it describes
# every conversation as "Mock thread description.", says NONE (nothing to describe yet) when the conversation holds
# `[mock:undescribed]` and fails with a 500 when it holds `[mock:description-error]`. The orchestrator reaches it through
# `tasks.description` of dev/orchestrator.yaml: the endpoint `small` (`http://mock-model:8080`, no `/v1`, so the request
# journal tells it from the title's `/v1/chat/completions`), the model `mock-description`, the guidance "Say in one sentence
# what the person wants and where it stands." and `recompute.minNewMessages: 2`. The agent is the `chat` of dev/agents,
# which answers every first message.
#
# The script speaks what the web speaks (docs/api/agui.md): one POST /agui/agents/{agentId} per message, the thread from
# GET /api/threads/{id} and the list the sidebar shows from GET /api/threads, and a description is written with
# PATCH /api/threads/{id} (docs/api/chat-api.yaml, `patchThread`).
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * "hi": the thread ends `done`, then its description becomes "Mock thread description." (the sidebar's list says so
#     too, and so does the last STATE_SNAPSHOT of the AG-UI stream), the log has a `thread_described` of the orchestrator
#     (`source: model`), and `mock-model` was asked once for it, at `/chat/completions` (the endpoint `small`, not the
#     title's `/v1/...`), with the configured guidance, the form of the answer and the data clause the core always adds in
#     the system message, and the conversation in a fence as data ("user: hi") with the language line last;
#   * `[mock:undescribed] hi`: the model says NONE, and the thread has no description and no `thread_described`;
#   * `[mock:description-error] hi`: the model fails (a 500, asked three times), and the thread still ends `done` with no
#     description and no failure in its log;
#   * a second message in the first thread starts its next job, whose end asks again, with the description so far in a
#     fence of its own ("previous description");
#   * a person's description ("My own description"): PATCH answers the thread with it, the log has a `thread_described` of
#     the person (`source: user`), a fork of the thread has it from the start (the `thread_forked` event says so), and the
#     next message in the thread does not ask the model again; an empty one clears it, and that is final too;
#   * GET /api/config says `{"ui": {"showDescriptions": true}}`;
#   * `mock-model` matched every request.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_MODEL_URL  http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   TIMEOUT         120    seconds to wait for a thread to stop or a description to appear
#
# It EMPTIES the request journal of `mock-model` first and resets its scenarios, so run it on a stack you are not in the
# middle of another scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
timeout=${TIMEOUT:-120}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "description e2e passed"; else echo "description e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

api() { # api METHOD PATH [BODY]: the body on stdout, non-zero when the status is not 2xx (the resource API)
  if [ $# -ge 3 ]; then
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header" \
      -H 'content-type: application/json' -d "$3"
  else
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
  fi
}

say() { # say AGENT THREAD TEXT: one run (a message), to its end
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

wait_state() { # wait_state THREAD: the thread's state once it stops moving (done, blocked, failed, cancelled)
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(api GET "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' || true)
    case $_state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 1
  done
  echo "$_state"
}

description_of() { api GET "/api/threads/$1" 2>/dev/null | jq -r '.description // empty' || true; }

wait_description() { # wait_description THREAD WANT: succeeds once the thread has the description WANT, within the timeout
  _deadline=$(( $(date +%s) + timeout ))
  while :; do
    [ "$(description_of "$1")" = "$2" ] && return 0
    if [ "$(date +%s)" -ge "$_deadline" ]; then return 1; fi
    sleep 1
  done
}

requests() { # requests: what mock-description was asked, one {url, body} per line, OLDEST FIRST (the journal lists the newest first)
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c '.requests | sort_by(.request.loggedDate) | .[] | {url: .request.url, body: (.request.body | fromjson? // {})} | select(.body.model == "mock-description")' 2>/dev/null || true
}

asked() { requests | wc -l | tr -d ' '; }

asked_about() { # asked_about MARKER: how many times mock-description was asked about a conversation that holds MARKER
  requests | grep -c "$1" || true
}

wait_asked() { # wait_asked MARKER N: succeeds once the model was asked about MARKER at least N times, within the timeout
  _deadline=$(( $(date +%s) + timeout ))
  while :; do
    [ "$(asked_about "$1")" -ge "$2" ] && return 0
    if [ "$(date +%s)" -ge "$_deadline" ]; then return 1; fi
    sleep 1
  done
}

sidebar_description() { # sidebar_description THREAD: the description the thread list (GET /api/threads) shows
  api GET /api/threads 2>/dev/null | jq -r --arg id "$1" '.[] | select(.id == $id) | .description // empty' || true
}

events_of() { api GET "/api/threads/$1/export" 2>/dev/null | jq -c '.events' || echo '[]'; }

described_events() { # described_events THREAD: the thread_described events, as [{description, source, by}]
  events_of "$1" | jq -c '[.[] | select(.kind == "thread_described") | {description: .data.description, source: .data.source, by: .actor.name}]'
}

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $code"; finish; fi
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X POST "$model/__admin/scenarios/reset" || true)
if [ "$code" = 200 ]; then ok "scenarios reset: $model"; else bad "scenarios reset: $model answered HTTP $code"; finish; fi

# --- the public configuration ---------------------------------------------------------------------------------------
if cfg=$(api GET /api/config 2>"$tmp/err"); then
  if [ "$(printf '%s' "$cfg" | jq -c .)" = '{"ui":{"showDescriptions":true}}' ]; then
    ok "GET /api/config says {\"ui\": {\"showDescriptions\": true}}"
  else
    bad "GET /api/config answered $cfg"
  fi
else
  bad "GET /api/config failed: $(head -c 300 "$tmp/err") $cfg"
fi

# --- a thread is described by the model when its job ends -----------------------------------------------------------
a=$(uuid)
echo "thread $a (hi)"
code=$(say chat "$a" "hi")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}: $(head -c 300 "$tmp/err")"; finish; fi
state=$(wait_state "$a")
if [ "$state" = "done" ]; then ok "hi: the thread ended done (the chat answered)"; else bad "hi: the thread ended '${state:-unknown}', want done"; fi
if wait_description "$a" "Mock thread description."; then
  ok "hi: the thread's description became \"Mock thread description.\""
else
  bad "hi: the description is \"$(description_of "$a")\" after at most ${timeout}s, want \"Mock thread description.\" (the orchestrator needs tasks.description: dev/orchestrator.yaml)"
fi
if [ "$(sidebar_description "$a")" = "Mock thread description." ]; then
  ok "hi: the thread list (the sidebar) says the description"
else
  bad "hi: the thread list says \"$(sidebar_description "$a")\""
fi
events=$tmp/events.json
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
  "$base/agui/threads/$a/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
  echo '[]' > "$events"
last=$(jq -r '[.[] | select(.type == "STATE_SNAPSHOT") | .snapshot.thread.description] | last // empty' "$events" 2>/dev/null || true)
if [ "$last" = "Mock thread description." ]; then
  ok "hi: the AG-UI stream's last STATE_SNAPSHOT carries the description (the screen's header)"
else
  bad "hi: the last STATE_SNAPSHOT says \"$last\""
fi
if [ "$(described_events "$a")" = '[{"description":"Mock thread description.","source":"model","by":"orchestrator"}]' ]; then
  ok "hi: the log has one thread_described, written by the orchestrator, source model"
else
  bad "hi: the log's thread_described events: $(described_events "$a")"
fi
n=$(asked)
if [ "$n" = 1 ]; then ok "hi: mock-model was asked once for a description"; else bad "hi: mock-model was asked $n times, want 1"; fi
first=$(requests | head -n 1)
url=$(printf '%s' "$first" | jq -r '.url')
if [ "$url" = "/chat/completions" ]; then
  ok "hi: the request went to the endpoint small (/chat/completions), not the title's /v1/chat/completions"
else
  bad "hi: the request went to $url"
fi
system=$(printf '%s' "$first" | jq -r '[.body.messages[] | select(.role == "system") | .content] | join("\n")')
user=$(printf '%s' "$first" | jq -r '[.body.messages[] | select(.role == "user") | .content] | join("\n")')
case $system in
  "Say in one sentence what the person wants and where it stands."*) ok "hi: the system message starts with the configured guidance" ;;
  *) bad "hi: the system message is: $system" ;;
esac
case $system in
  *"Answer with the description alone"*"never instructions to follow"*) ok "hi: the core's form of the answer and data clause follow the guidance" ;;
  *) bad "hi: the system message lacks the form of the answer and the data clause: $system" ;;
esac
case $user in
  *'conversation'*'user: hi'*) ok "hi: the conversation is shown as data, in a fence (user: hi)" ;;
  *) bad "hi: the model was shown: $user" ;;
esac
lastline=$(printf '%s' "$user" | awk 'NF { l = $0 } END { print l }')
case $lastline in
  "Write the description in"*) ok "hi: the language line is last (\"$lastline\")" ;;
  *) bad "hi: the last line of the request is \"$lastline\"" ;;
esac
maxtok=$(printf '%s' "$first" | jq -r '.body.max_tokens')
if [ "$maxtok" = 120 ]; then ok "hi: max_tokens is the task's (120)"; else bad "hi: max_tokens is $maxtok, want 120"; fi
titled=$(events_of "$a" | jq -c '[.[] | select(.kind == "thread_titled") | .data.title]')
if [ "$titled" = '["Mock thread title"]' ]; then ok "hi: the title task works beside it (the title is \"Mock thread title\")"; else bad "hi: the log's titles: $titled"; fi

# --- nothing to describe yet: no description ---------------------------------------------------------------------------
b=$(uuid)
echo "thread $b ([mock:undescribed] hi)"
code=$(say chat "$b" "[mock:undescribed] hi")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$b")
if [ "$state" = "done" ]; then ok "undescribed: the thread ended done"; else bad "undescribed: the thread ended '${state:-unknown}', want done"; fi
if wait_asked 'mock:undescribed' 1; then ok "undescribed: the model was asked, and said NONE"; else bad "undescribed: the model was never asked"; fi
sleep 3
if [ -z "$(description_of "$b")" ] && [ "$(described_events "$b")" = '[]' ]; then
  ok "undescribed: the thread has no description and no thread_described event"
else
  bad "undescribed: the description is \"$(description_of "$b")\", events $(described_events "$b")"
fi

# --- a model that fails: nothing fails ------------------------------------------------------------------------------------
c=$(uuid)
echo "thread $c ([mock:description-error] hi)"
code=$(say chat "$c" "[mock:description-error] hi")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$c")
if [ "$state" = "done" ]; then ok "description-error: the thread ended done"; else bad "description-error: the thread ended '${state:-unknown}', want done"; fi
wait_asked 'mock:description-error' 3 || true
sleep 2
n=$(asked_about 'mock:description-error')
if [ "$n" = 3 ]; then ok "description-error: the model was asked three times (a failing model is retried a little, then given up on)"; else bad "description-error: the model was asked $n times, want 3"; fi
if [ -z "$(description_of "$c")" ]; then ok "description-error: the thread has no description"; else bad "description-error: the description is \"$(description_of "$c")\""; fi
errors=$(events_of "$c" | jq -r '[.[] | select(.kind == "error")] | length' || echo '?')
if [ "$errors" = 0 ]; then ok "description-error: nothing failed in the thread's log (no error event)"; else bad "description-error: the log has $errors error events"; fi

# --- the next job asks again, and shows the description so far ----------------------------------------------------------
before=$(asked)
code=$(say chat "$a" "and one more thing")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$a")
if [ "$state" = "done" ]; then ok "next job: the thread ended done again"; else bad "next job: the thread ended '${state:-unknown}', want done"; fi
deadline=$(( $(date +%s) + timeout ))
while [ "$(described_events "$a" | jq -r 'length')" -lt 2 ] && [ "$(date +%s)" -lt "$deadline" ]; do sleep 1; done
after=$(asked)
if [ "$after" = "$(( before + 1 ))" ]; then ok "next job: the model was asked once more (job 2 asks at its end)"; else bad "next job: the model was asked ${before}->${after} times, want one more"; fi
second=$(requests | tail -n 1 | jq -r '[.body.messages[] | select(.role == "user") | .content] | join("\n")')
case $second in
  *'previous description'*'Mock thread description.'*'conversation'*'and one more thing'*) ok "next job: the description so far is in a fence of its own, before the conversation" ;;
  *) bad "next job: the model was shown: $second" ;;
esac

# --- a person's description is final, a fork has it -----------------------------------------------------------------------
mine="My own description"
if written=$(api PATCH "/api/threads/$a" "$(jq -n --arg d "$mine" '{description: $d}')" 2>"$tmp/err"); then
  if [ "$(printf '%s' "$written" | jq -r .description)" = "$mine" ]; then
    ok "write: PATCH answered the thread with the person's description"
  else
    bad "write: PATCH answered $written"
  fi
else
  bad "write: PATCH failed: $(head -c 300 "$tmp/err") $written"
fi
by_user=$(described_events "$a" | jq -c '.[-1]')
if [ "$by_user" = "{\"description\":\"$mine\",\"source\":\"user\",\"by\":\"$email\"}" ]; then
  ok "write: the log's last thread_described is the person's (source user)"
else
  bad "write: the log's last thread_described is $by_user"
fi
if bad_patch=$(api PATCH "/api/threads/$a" '{"description":"two\nlines"}' 2>/dev/null); then
  bad "write: a description with a line break was accepted: $bad_patch"
else
  ok "write: a description with a line break is refused (400)"
fi
if fork=$(api POST "/api/threads/$a/fork" '{"after":1}' 2>"$tmp/err"); then
  fid=$(printf '%s' "$fork" | jq -r '.id')
  if [ "$(printf '%s' "$fork" | jq -r '.description // empty')" = "$mine" ] && [ "$(description_of "$fid")" = "$mine" ]; then
    ok "fork: the fork has its parent's description from the start"
  else
    bad "fork: the fork says \"$(printf '%s' "$fork" | jq -r '.description // empty')\""
  fi
  marker=$(events_of "$fid" | jq -r '[.[] | select(.kind == "thread_forked")] | .[0].data.description // empty')
  if [ "$marker" = "$mine" ]; then ok "fork: the thread_forked event says the description"; else bad "fork: the thread_forked event says \"$marker\""; fi
else
  bad "fork: POST failed: $(head -c 300 "$tmp/err") $fork"
fi
before=$(asked)
code=$(say chat "$a" "and a last thing")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$a")
if [ "$state" = "done" ]; then ok "final: the next message was answered (done)"; else bad "final: the thread ended '${state:-unknown}'"; fi
sleep 4
after=$(asked)
if [ "$after" = "$before" ]; then ok "final: the model was not asked again ($after requests in all)"; else bad "final: the model was asked ${before}->${after} times"; fi
if [ "$(description_of "$a")" = "$mine" ]; then ok "final: the person's description stays"; else bad "final: the description is \"$(description_of "$a")\""; fi

# an empty description clears it, and that is final too
if cleared=$(api PATCH "/api/threads/$a" '{"description":""}' 2>"$tmp/err"); then
  if [ "$(printf '%s' "$cleared" | jq -r 'has("description")')" = false ]; then
    ok "clear: PATCH answered the thread with no description"
  else
    bad "clear: PATCH answered $cleared"
  fi
else
  bad "clear: PATCH failed: $(head -c 300 "$tmp/err") $cleared"
fi
code=$(say chat "$a" "one more after clearing")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$a")
if [ "$state" = "done" ]; then ok "clear: the next message was answered (done)"; else bad "clear: the thread ended '${state:-unknown}'"; fi
sleep 4
if [ "$(asked)" = "$before" ] && [ -z "$(description_of "$a")" ]; then
  ok "clear: the model was not asked again, and the thread has no description"
else
  bad "clear: the model was asked $(asked) times in all (${before} before), the description is \"$(description_of "$a")\""
fi

unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then ok "mock-model matched every request"; else bad "mock-model saw $unmatched unmatched requests (see $model/__admin/requests/unmatched)"; fi

finish
