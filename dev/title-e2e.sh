#!/usr/bin/env sh
# System-level test of the orchestrator's own model call (ADR 0005, MVP slice 6): after the agent's first reply
# the thread is given a short title by a model, and a person's rename is final.
#
#   dev/title-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; the `chat` agent runs from
# the same image):
#
#   docker compose --profile app up -d --build --wait
#
# The model is `mock-title` on the WireMock `mock-model` (dev/wiremock/model/mappings/title.json): it titles every
# conversation "Mock thread title", says NONE (no topic yet) when the conversation holds `[mock:untitled]`, fails
# with a 500 when it holds `[mock:title-error]`, answers in Chinese the first time it is asked about a conversation that
# holds `[mock:title-chinese]` (and "Mock thread title" after) and always in Chinese for one that holds `[mock:title-zh]`. The orchestrator reaches it through ORCH_MODEL_BASE_URL and
# ORCH_TITLE_MODEL (compose.yaml). The agent is the `chat` of dev/agents, which answers every first message.
#
# The script speaks what the web speaks (docs/api/agui.md): one POST /agui/agents/{agentId} per message, the
# thread from GET /api/threads/{id} and the list the sidebar shows from GET /api/threads, and a rename is
# PATCH /api/threads/{id} (docs/api/chat-api.yaml, `patchThread`).
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * "hi": the thread ends `done`, then its title becomes "Mock thread title" (the sidebar's list says so too),
#     the log has a `thread_titled` of the orchestrator (`source: model`), and `mock-model` was asked once, with the
#     conversation in a fence as data ("user: hi") and the instruction for a 3 to 6 word title;
#   * `[mock:untitled] hi`: the model says NONE, and the thread keeps the first words of its first message;
#   * `[mock:title-error] hi`: the model fails (a 500, asked three times), and the thread still ends `done`
#     with the first words as its title, and no failure in its log;
#   * `[mock:title-chinese] Please fix the login page ...` (English): the model drifts and answers in Chinese the first
#     time (a WireMock scenario, once). The core declines a title in a script the person never wrote, the dispatcher asks
#     again with the language named once more, and the thread's title is the second answer, never the Chinese one:
#     the model was asked twice, the first time with "Write the title in English." as the last line;
#   * `[mock:title-zh] 请修复登录页面的重定向问题` (Chinese): the model answers in Chinese and the title is kept (a title in the
#     person's own script is not declined), asked once, with "Write the title in Chinese." as the last line;
#   * a rename ("My own title") of the first thread, then another message in it: the title stays, and the model is
#     not asked again; the same for the `[mock:untitled]` thread (the person's title wins over a model that said NONE).
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_MODEL_URL  http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   TIMEOUT         120    seconds to wait for a thread to stop or a title to appear
#
# It EMPTIES the request journal of `mock-model` first and resets its scenarios (the Chinese answer is given once), so run it on a stack you are not in the middle of another
# scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
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
  if [ "$fail" -eq 0 ]; then echo "title e2e passed"; else echo "title e2e FAILED"; exit 1; fi
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

title_of() { api GET "/api/threads/$1" 2>/dev/null | jq -r '.title // empty' || true; }

wait_title() { # wait_title THREAD WANT: succeeds once the thread has the title WANT, within the timeout
  _deadline=$(( $(date +%s) + timeout ))
  while :; do
    [ "$(title_of "$1")" = "$2" ] && return 0
    if [ "$(date +%s)" -ge "$_deadline" ]; then return 1; fi
    sleep 1
  done
}

requests() { # requests: what mock-title was asked, one JSON body per line, OLDEST FIRST (the journal lists the newest first)
  curl -s --max-time 30 "$model/__admin/requests" |
    jq -c '.requests | sort_by(.request.loggedDate) | .[].request.body | fromjson? | select(.model == "mock-title")' 2>/dev/null || true
}

asked() { requests | wc -l | tr -d ' '; }

sidebar_title() { # sidebar_title THREAD: the title the thread list (GET /api/threads) shows
  api GET /api/threads 2>/dev/null | jq -r --arg id "$1" '.[] | select(.id == $id) | .title' || true
}

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $model"; else bad "journal reset: $model answered HTTP $code"; finish; fi
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X POST "$model/__admin/scenarios/reset" || true)
if [ "$code" = 200 ]; then ok "scenarios reset: $model"; else bad "scenarios reset: $model answered HTTP $code"; finish; fi

# --- a thread is titled by the model after the agent's first reply --------------------------------------
a=$(uuid)
echo "thread $a (hi)"
code=$(say chat "$a" "hi")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}: $(head -c 300 "$tmp/err")"; finish; fi
state=$(wait_state "$a")
if [ "$state" = "done" ]; then ok "hi: the thread ended done (the chat answered)"; else bad "hi: the thread ended '${state:-unknown}', want done"; fi
if wait_title "$a" "Mock thread title"; then
  ok "hi: the thread's title became \"Mock thread title\""
else
  bad "hi: the title is \"$(title_of "$a")\" after at most ${timeout}s, want \"Mock thread title\" (the orchestrator needs ORCH_TITLE_MODEL and ORCH_MODEL_BASE_URL: compose.yaml)"
fi
if [ "$(sidebar_title "$a")" = "Mock thread title" ]; then
  ok "hi: the thread list (the sidebar) says the new title"
else
  bad "hi: the thread list says \"$(sidebar_title "$a")\""
fi
events=$tmp/events.json
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
  "$base/agui/threads/$a/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
  echo '[]' > "$events"
last=$(jq -r '[.[] | select(.type == "STATE_SNAPSHOT") | .snapshot.thread.title] | last // empty' "$events" 2>/dev/null || true)
if [ "$last" = "Mock thread title" ]; then
  ok "hi: the AG-UI stream's last STATE_SNAPSHOT carries the title (the screen's header)"
else
  bad "hi: the last STATE_SNAPSHOT says \"$last\""
fi
export=$(api GET "/api/threads/$a/export" 2>/dev/null || true)
titled=$(printf '%s' "$export" | jq -c '[.events[] | select(.kind == "thread_titled")] | map({title: .data.title, source: .data.source, by: .actor.name}) ' 2>/dev/null || true)
if [ "$titled" = '[{"title":"Mock thread title","source":"model","by":"orchestrator"}]' ]; then
  ok "hi: the log has one thread_titled, written by the orchestrator, source model"
else
  bad "hi: the log's thread_titled events: $titled"
fi
n=$(asked)
if [ "$n" = 1 ]; then ok "hi: mock-model was asked once"; else bad "hi: mock-model was asked $n times, want 1"; fi
prompt=$(requests | jq -rs '.[0] | [.messages[] | select(.role == "system") | .content] | join(" ")')
user=$(requests | jq -rs '.[0] | [.messages[] | select(.role == "user") | .content] | join(" ")')
case $prompt in
  *"3 to 6 word title"*) ok "hi: the instruction asks for a 3 to 6 word title" ;;
  *) bad "hi: the instruction is: $prompt" ;;
esac
case $user in
  *'conversation'*'user: hi'*) ok "hi: the conversation is shown as data, in a fence (user: hi)" ;;
  *) bad "hi: the model was shown: $user" ;;
esac

# --- a model with no topic yet: the first words stay --------------------------------------------------------
b=$(uuid)
echo "thread $b ([mock:untitled] hi)"
code=$(say chat "$b" "[mock:untitled] hi")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$b")
if [ "$state" = "done" ]; then ok "untitled: the thread ended done"; else bad "untitled: the thread ended '${state:-unknown}', want done"; fi
deadline=$(( $(date +%s) + timeout ))
while [ "$(requests | grep -c 'mock:untitled' || true)" -lt 1 ] && [ "$(date +%s)" -lt "$deadline" ]; do sleep 1; done
sleep 2
if [ "$(requests | grep -c 'mock:untitled' || true)" -ge 1 ]; then ok "untitled: the model was asked, and said NONE"; else bad "untitled: the model was never asked"; fi
if [ "$(title_of "$b")" = "[mock:untitled] hi" ]; then
  ok "untitled: the thread keeps the first words of its first message"
else
  bad "untitled: the title is \"$(title_of "$b")\", want the first message"
fi

# --- a model that fails: nothing fails ------------------------------------------------------------------------
c=$(uuid)
echo "thread $c ([mock:title-error] hi)"
code=$(say chat "$c" "[mock:title-error] hi")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$c")
if [ "$state" = "done" ]; then ok "title-error: the thread ended done"; else bad "title-error: the thread ended '${state:-unknown}', want done"; fi
deadline=$(( $(date +%s) + timeout ))
while [ "$(requests | grep -c 'mock:title-error' || true)" -lt 3 ] && [ "$(date +%s)" -lt "$deadline" ]; do sleep 1; done
n=$(requests | grep -c 'mock:title-error' || true)
if [ "$n" = 3 ]; then ok "title-error: the model was asked three times (a failing model is retried a little, then given up on)"; else bad "title-error: the model was asked $n times, want 3"; fi
sleep 2
if [ "$(title_of "$c")" = "[mock:title-error] hi" ]; then
  ok "title-error: the thread keeps the first words of its first message"
else
  bad "title-error: the title is \"$(title_of "$c")\""
fi
errors=$(api GET "/api/threads/$c/export" 2>/dev/null | jq -r '[.events[] | select(.kind == "error")] | length' || echo '?')
if [ "$errors" = 0 ]; then ok "title-error: nothing failed in the thread's log (no error event)"; else bad "title-error: the log has $errors error events"; fi

# --- a title in a script the person did not write is declined, and the next ask names the language ----------------
d=$(uuid)
echo "thread $d ([mock:title-chinese], English)"
code=$(say chat "$d" "[mock:title-chinese] Please fix the login page and the redirect to the dashboard")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$d")
if [ "$state" = "done" ]; then ok "chinese: the thread ended done"; else bad "chinese: the thread ended '${state:-unknown}', want done"; fi
if wait_title "$d" "Mock thread title"; then
  ok "chinese: the thread's title is the model's second answer (\"Mock thread title\"), not its Chinese one"
else
  bad "chinese: the title is \"$(title_of "$d")\" after at most ${timeout}s, want \"Mock thread title\""
fi
n=$(requests | grep -c 'mock:title-chinese' || true)
if [ "$n" = 2 ]; then ok "chinese: the model was asked twice (the Chinese title was declined, then asked again)"; else bad "chinese: the model was asked $n times, want 2"; fi
last_line() { # last_line N: the last line of the user message of the N-th (from 1) request that holds the marker
  requests | grep 'mock:title-chinese' | jq -rs --argjson n "$1" '.[$n - 1] | [.messages[] | select(.role == "user") | .content] | join("\n") | split("\n") | map(select(length > 0)) | last'
}
first=$(last_line 1)
second=$(last_line 2)
if [ "$first" = "Write the title in English." ]; then
  ok "chinese: the first ask names the language last (\"$first\")"
else
  bad "chinese: the last line of the first ask is \"$first\", want \"Write the title in English.\""
fi
if [ "$second" = "Write the title in English." ]; then
  ok "chinese: the second ask names the language last again (\"$second\")"
else
  bad "chinese: the last line of the second ask is \"$second\", want \"Write the title in English.\""
fi
retry=$(requests | grep 'mock:title-chinese' | jq -rs '.[1] | [.messages[] | select(.role == "user") | .content] | join("\n")')
case $retry in
  *"script the person did not write in"*) ok "chinese: the second ask says what was wrong with the first answer" ;;
  *) bad "chinese: the second ask is: $retry" ;;
esac
titled=$(api GET "/api/threads/$d/export" 2>/dev/null | jq -c '[.events[] | select(.kind == "thread_titled") | .data.title]' || true)
if [ "$titled" = '["Mock thread title"]' ]; then ok "chinese: the log has one thread_titled, and the Chinese title is in none"; else bad "chinese: the log's titles: $titled"; fi

# --- a title in the person's own script is kept ----------------------------------------------------------------------
e=$(uuid)
echo "thread $e ([mock:title-zh], Chinese)"
code=$(say chat "$e" "[mock:title-zh] 请修复登录页面的重定向问题")
if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; finish; fi
state=$(wait_state "$e")
if [ "$state" = "done" ]; then ok "zh: the thread ended done"; else bad "zh: the thread ended '${state:-unknown}', want done"; fi
if wait_title "$e" "登录页面修复"; then
  ok "zh: the Chinese conversation keeps its Chinese title"
else
  bad "zh: the title is \"$(title_of "$e")\" after at most ${timeout}s, want \"登录页面修复\""
fi
n=$(requests | grep -c 'mock:title-zh' || true)
if [ "$n" = 1 ]; then ok "zh: the model was asked once"; else bad "zh: the model was asked $n times, want 1"; fi
last=$(requests | grep 'mock:title-zh' | jq -rs '.[0] | [.messages[] | select(.role == "user") | .content] | join("\n") | split("\n") | map(select(length > 0)) | last')
if [ "$last" = "Write the title in Chinese." ]; then ok "zh: the ask names the language last (\"$last\")"; else bad "zh: the last line of the ask is \"$last\""; fi

# --- a person's title is final ----------------------------------------------------------------------------------
before=$(asked)
for t in "$a" "$b"; do
  if renamed=$(api PATCH "/api/threads/$t" '{"title":"My own title"}' 2>"$tmp/err"); then
    if [ "$(printf '%s' "$renamed" | jq -r .title)" = "My own title" ]; then ok "rename $t: PATCH answered the thread with the new title"; else bad "rename $t: PATCH answered $renamed"; fi
  else
    bad "rename $t: PATCH failed: $(head -c 300 "$tmp/err") $renamed"
  fi
  code=$(say chat "$t" "and one more thing")
  if [ "$code" != 200 ]; then bad "POST /agui/agents/chat answered HTTP ${code:-none}"; continue; fi
  state=$(wait_state "$t")
  if [ "$state" = "done" ]; then ok "rename $t: the next message was answered (done)"; else bad "rename $t: the thread ended '${state:-unknown}'"; fi
done
sleep 4
after=$(asked)
if [ "$after" = "$before" ]; then
  ok "renamed threads: the model was not asked again ($after requests in all)"
else
  bad "renamed threads: the model was asked ${before}->${after} times"
fi
for t in "$a" "$b"; do
  if [ "$(title_of "$t")" = "My own title" ]; then ok "renamed $t: the person's title stays"; else bad "renamed $t: the title is \"$(title_of "$t")\""; fi
done
unmatched=$(curl -s --max-time 30 "$model/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then ok "mock-model matched every request"; else bad "mock-model saw $unmatched unmatched requests (see $model/__admin/requests/unmatched)"; fi

finish
