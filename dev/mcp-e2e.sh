#!/usr/bin/env sh
# System-level test of the MCP server (ADR 0019): an MCP client starts a job on the mock agent and
# follows it to `done`, through the compose `edge`, with a bearer token.
#
#   docker compose --profile app up -d --build --wait
#   dev/mcp-e2e.sh
#
# The script speaks MCP streamable HTTP by hand, as any client does (stateless: no session id), and
# prints one ok or FAIL line per check; it exits 1 if any failed:
#   * a request without a token is 401 with `WWW-Authenticate: Bearer`, and so is a wrong token;
#   * `initialize` answers with the server's tools capability and no `Mcp-Session-Id`;
#   * `tools/list` gives the six tools;
#   * `list_agents` lists the agent the job is given to;
#   * `start_job` returns a job id, its state and (the compose stack sets ORCH_PUBLIC_URL) a web_url;
#     the same `client_request_id` again returns the same job, not a second one (with another text it is refused);
#   * `get_job`, polled, ends `done` and names the pull request the mock agent opened;
#   * the chat's resource API shows the same thread `done` for the token's user, so the job is in
#     the chat too;
#   * an unknown job is "no such job", a message to the finished job is refused and a cancel of it
#     is a no-op;
#   * `wait_for_job` with a progress token, on the mock agent's `slow` script (8 s): the response is an
#     event stream, `notifications/progress` come with a counter that only increases, one per event of
#     the job, and the last message is the finished job with its pull request;
#   * a `wait_for_job` that times out (`timeout_secs: 0`) says where to resume, and the call that resumes
#     there reports the rest: together the two calls name every event of the job exactly once.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL     http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge` (which adds no identity to /mcp)
#   MCP_TOKEN    dev-mcp-token-0123456789abcdef0123456789, the dummy bearer token of dev/mcp-tokens.yaml
#   AUTH_EMAIL   dev@example.com, the user of that token, sent as X-Auth-Request-Email to the chat API
#   AGENT_ID     mock-coder, the mock A2A agent that finishes with a pull request
#   TIMEOUT      120    seconds to wait for the job
#
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
token=${MCP_TOKEN:-dev-mcp-token-0123456789abcdef0123456789}
email=${AUTH_EMAIL:-dev@example.com}
agent_id=${AGENT_ID:-mock-coder}
timeout=${TIMEOUT:-120}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "mcp e2e passed"; else echo "mcp e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# post TOKEN BODY: one JSON-RPC POST to /mcp. The headers go to $tmp/h, the body to $tmp/b, and the
# HTTP status is printed. An empty TOKEN sends no Authorization header.
post() {
  if [ -n "$1" ]; then auth="Authorization: Bearer $1"; else auth="X-No-Authorization: none"; fi
  curl -sS --max-time 180 -o "$tmp/b" -D "$tmp/h" -w '%{http_code}' -X POST "$base/mcp" \
    -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
    -H "$auth" -d "$2" 2>"$tmp/err" || true
}

# message ID: the JSON-RPC message of $tmp/b that answers request ID. The body is either JSON or a
# server-sent event stream (possibly with progress notifications before the answer).
message() {
  case $(head -c 1 "$tmp/b") in
    '{') jq -c --argjson id "$1" 'select(.id == $id)' "$tmp/b" ;;
    *) sed -n 's/^data: *//p' "$tmp/b" | jq -c --argjson id "$1" 'select(.id == $id)' ;;
  esac
}

# rpc ID METHOD PARAMS: a request with a token; the answer is on stdout, "-" when there is none.
rpc() {
  body=$(jq -cn --argjson id "$1" --arg method "$2" --argjson params "$3" \
    '{jsonrpc: "2.0", id: $id, method: $method, params: $params}')
  code=$(post "$token" "$body")
  if [ "$code" != 200 ]; then echo "-"; return 0; fi
  answer=$(message "$1")
  if [ -n "$answer" ]; then printf '%s\n' "$answer"; else echo "-"; fi
}

n=10
# call TOOL ARGS: a tools/call. The tool's result (its structured content, or {text} for a failure)
# is left in $result and is_error says whether the tool reported a failure. It sets variables, so it
# is not called in a command substitution.
call() {
  n=$((n + 1))
  answer=$(rpc "$n" tools/call "$(jq -cn --arg name "$1" --argjson arguments "$2" '{name: $name, arguments: $arguments}')")
  is_error=$(printf '%s' "$answer" | jq -r '.result.isError // false' 2>/dev/null || echo true)
  result=$(printf '%s' "$answer" | jq -c '.result.structuredContent // {text: (.result.content[0].text // .error.message // "no answer")}' 2>/dev/null || echo '{}')
}

# --- authentication ----------------------------------------------------------------------------
list='{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
for who in "" "not-the-token"; do
  code=$(post "$who" "$list")
  challenge=$(grep -i '^www-authenticate:' "$tmp/h" | tr -d '\r' | sed 's/^[^:]*: *//' || true)
  if [ "$code" = 401 ] && [ "$challenge" = Bearer ]; then
    ok "${who:-no token}: 401 with WWW-Authenticate: Bearer"
  else
    bad "${who:-no token}: HTTP $code, challenge '$challenge' (want 401 and Bearer)"
  fi
done

# --- the handshake -----------------------------------------------------------------------------
init=$(jq -cn '{jsonrpc: "2.0", id: 2, method: "initialize", params: {protocolVersion: "2025-06-18",
  capabilities: {}, clientInfo: {name: "mcp-e2e.sh", version: "0"}}}')
code=$(post "$token" "$init")
if [ "$code" = 200 ] && [ "$(message 2 | jq -r '.result.capabilities.tools | type')" = object ]; then
  ok "initialize: the server offers tools"
else
  bad "initialize: HTTP $code $(head -c 300 "$tmp/b") $(head -c 200 "$tmp/err")"
  finish
fi
if grep -qi '^mcp-session-id:' "$tmp/h"; then
  bad "initialize gave a session id: the server is not stateless"
else
  ok "initialize: no session id (stateless: any replica can serve any call)"
fi
code=$(post "$token" '{"jsonrpc":"2.0","method":"notifications/initialized"}')
if [ "$code" = 202 ]; then ok "notifications/initialized accepted"; else bad "notifications/initialized: HTTP $code"; fi

tools=$(rpc 3 tools/list '{}' | jq -r '[.result.tools[]?.name] | join(" ")')
want="list_agents start_job get_job wait_for_job answer cancel_job"
if [ "$tools" = "$want" ]; then ok "tools/list: $tools"; else bad "tools/list: '$tools', want '$want'"; fi

# --- a job -------------------------------------------------------------------------------------
call list_agents '{}'
agents=$(printf '%s' "$result" | jq -r '[.agents[]?.id] | join(" ")')
case " $agents " in
  *" $agent_id "*) ok "list_agents: $agents" ;;
  *) bad "list_agents: '$agents' lacks $agent_id" ;;
esac

request_id=$(uuid)
call start_job "$(jq -cn --arg agent "$agent_id" --arg id "$request_id" \
  '{text: "add a health endpoint", agent: $agent, client_request_id: $id}')"
started=$result
job=$(printf '%s' "$started" | jq -r '.job_id // empty')
if [ -n "$job" ] && [ "$is_error" = false ]; then
  ok "start_job: job $job ($(printf '%s' "$started" | jq -r '.state'))"
else
  bad "start_job: $started"
  finish
fi
if printf '%s' "$started" | jq -e --arg job "$job" '.created == true and (.web_url | endswith("/threads/" + $job))' >/dev/null 2>&1; then
  ok "start_job: created, with a web_url"
else
  bad "start_job: expected created and a web_url ending in /threads/$job, got $started"
fi
call start_job "$(jq -cn --arg agent "$agent_id" --arg id "$request_id" \
  '{text: "add a health endpoint", agent: $agent, client_request_id: $id}')"
retried=$result
if [ "$(printf '%s' "$retried" | jq -r '.job_id')" = "$job" ] && [ "$(printf '%s' "$retried" | jq -r '.created')" = false ]; then
  ok "start_job again with the same client_request_id: the same job, not created"
else
  bad "start_job again: $retried, want job $job and created false"
fi
call start_job "$(jq -cn --arg agent "$agent_id" --arg id "$request_id" \
  '{text: "something else entirely", agent: $agent, client_request_id: $id}')"
if [ "$is_error" = true ] && printf '%s' "$result" | grep -q 'client_request_id'; then
  ok "start_job with the same client_request_id and another text: refused"
else
  bad "start_job with another text: $result, want a refusal naming client_request_id"
fi

# Poll get_job until the job is over. `blocked` is not final for a job, but nothing here answers it.
deadline=$(( $(date +%s) + timeout ))
state=
summary='{}'
while :; do
  call get_job "$(jq -cn --arg job "$job" '{job_id: $job}')"
  summary=$result
  state=$(printf '%s' "$summary" | jq -r '.state // empty')
  case $state in done | blocked | failed | cancelled) break ;; esac
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 1
done
if [ "$state" = "done" ]; then ok "get_job: the job ended done"; else bad "get_job: state '${state:-unknown}' after at most ${timeout}s: $summary"; fi
pr=$(printf '%s' "$summary" | jq -r '.pull_request.url // empty')
case $pr in
  https://*) ok "get_job: the pull request is $pr" ;;
  *) bad "get_job: no pull request in $summary" ;;
esac

# --- the same job in the chat ------------------------------------------------------------------
chat_state=$(curl -sS --max-time 30 -H "X-Auth-Request-Email: $email" "$base/api/threads/$job" 2>/dev/null | jq -r '.state // empty' || true)
if [ "$chat_state" = "done" ]; then
  ok "the chat's resource API shows the job done for $email"
else
  bad "the chat's resource API shows '${chat_state:-nothing}' for $email, want done"
fi

# --- refusals ----------------------------------------------------------------------------------
call get_job '{"job_id":"00000000-0000-7000-8000-00000000dead"}'
missing=$result
if [ "$is_error" = true ] && [ "$(printf '%s' "$missing" | jq -r '.text')" = "no such job" ]; then
  ok "an unknown job is 'no such job'"
else
  bad "an unknown job: error=$is_error $missing"
fi
call answer "$(jq -cn --arg job "$job" '{job_id: $job, text: "one more thing"}')"
late=$result
if [ "$is_error" = true ]; then ok "answer to the finished job is refused ($(printf '%s' "$late" | jq -r '.text'))"; else bad "answer to the finished job: $late"; fi
call cancel_job "$(jq -cn --arg job "$job" '{job_id: $job}')"
cancelled=$result
if [ "$is_error" = false ] && [ "$(printf '%s' "$cancelled" | jq -r '.state')" = "done" ]; then
  ok "cancel_job on the finished job is a no-op (still done)"
else
  bad "cancel_job on the finished job: error=$is_error $cancelled"
fi

# --- following a job: wait_for_job and its progress notifications --------------------------------
# The mock agent's `slow` script dribbles its answer over 8 s, so the job is still working when the
# wait starts. The progress token makes the server report each event as an SSE notification.
seqs_of() { # seqs_of FILE: the seq of each event a response reported, one per line
  sed -n 's/^data: *//p' "$1" | jq -r 'select(.method == "notifications/progress") | .params.message' | sed -n 's/^#\([0-9][0-9]*\) .*/\1/p'
}
call start_job "$(jq -cn --arg agent "$agent_id" '{text: "slow: add a health endpoint", agent: $agent}')"
slow=$(printf '%s' "$result" | jq -r '.job_id // empty')
if [ -z "$slow" ]; then bad "start_job (slow): $result"; finish; fi
wait_call='{"jsonrpc":"2.0","id":900,"method":"tools/call","params":{"name":"wait_for_job","_meta":{"progressToken":"e2e-wait-1"},"arguments":{"job_id":"JOB","after_seq":0,"timeout_secs":60}}}'
code=$(post "$token" "$(printf '%s' "$wait_call" | sed "s/JOB/$slow/")")
if [ "$code" = 200 ] && grep -qi '^content-type: text/event-stream' "$tmp/h"; then
  ok "wait_for_job: an event stream"
else
  bad "wait_for_job: HTTP $code, $(grep -i '^content-type:' "$tmp/h" | tr -d '\r')"
fi
sed -n 's/^data: *//p' "$tmp/b" | jq -c 'select(.method == "notifications/progress")' > "$tmp/notes"
notes=$(wc -l < "$tmp/notes" | tr -d ' ')
if [ "$notes" -ge 2 ] && jq -s -e 'all(.[]; .params.progressToken == "e2e-wait-1") and ([.[].params.progress] as $p | [range(1; length)] | all($p[.] > $p[. - 1])) and ([.[].params.progress] | first == 1)' "$tmp/notes" >/dev/null; then
  ok "wait_for_job: $notes progress notifications for the token, the counter increasing from 1"
else
  bad "wait_for_job: $notes progress notifications, or their token or counter is wrong: $(head -c 400 "$tmp/notes")"
fi
final=$(message 900 | jq -c '.result.structuredContent // {}')
last_reported=$(seqs_of "$tmp/b" | tail -n 1)
if [ "$(printf '%s' "$final" | jq -r '.outcome')" = finished ] && [ "$(printf '%s' "$final" | jq -r '.state')" = "done" ] &&
  [ -n "$last_reported" ] && [ "$(printf '%s' "$final" | jq -r '.last_seq')" = "$last_reported" ]; then
  ok "wait_for_job: the result is the finished job, and its last event (#$last_reported) was the last notification"
else
  bad "wait_for_job: the result is $final, the last notification named #${last_reported:-none}"
fi
case $(printf '%s' "$final" | jq -r '.pull_request.url // empty') in
  https://*) ok "wait_for_job: the result names the pull request" ;;
  *) bad "wait_for_job: no pull request in $final" ;;
esac

# A wait that times out says where to resume; the call that resumes there loses nothing.
call start_job "$(jq -cn --arg agent "$agent_id" '{text: "slow: add another endpoint", agent: $agent}')"
resumed=$(printf '%s' "$result" | jq -r '.job_id // empty')
first_call=$(post "$token" "$(printf '%s' "$wait_call" | sed -e "s/JOB/$resumed/" -e 's/"timeout_secs":60/"timeout_secs":0/' -e 's/e2e-wait-1/e2e-wait-2/' -e 's/"id":900/"id":901/')")
cp "$tmp/b" "$tmp/first"
first_result=$(message 901 | jq -c '.result.structuredContent // {}')
resume=$(printf '%s' "$first_result" | jq -r '.resume_after_seq // empty')
if [ "$first_call" = 200 ] && [ "$(printf '%s' "$first_result" | jq -r '.outcome')" = timed_out ] && [ -n "$resume" ]; then
  ok "wait_for_job with timeout_secs 0: timed_out, resume_after_seq $resume"
else
  bad "wait_for_job with timeout_secs 0: HTTP $first_call, $first_result"
fi
second_call=$(post "$token" "$(printf '%s' "$wait_call" | sed -e "s/JOB/$resumed/" -e "s/\"after_seq\":0/\"after_seq\":${resume:-0}/" -e 's/e2e-wait-1/e2e-wait-3/' -e 's/"id":900/"id":902/')")
cp "$tmp/b" "$tmp/second"
second_result=$(message 902 | jq -c '.result.structuredContent // {}')
together=$( (seqs_of "$tmp/first"; seqs_of "$tmp/second") | tr '\n' ' ' | sed 's/ *$//')
want_seqs=$(seq 1 "$(printf '%s' "$second_result" | jq -r '.last_seq // 0')" | tr '\n' ' ' | sed 's/ *$//')
if [ "$second_call" = 200 ] && [ "$(printf '%s' "$second_result" | jq -r '.outcome')" = finished ] && [ -n "$together" ] && [ "$together" = "$want_seqs" ]; then
  ok "resuming at $resume: finished, and the two calls named events $together exactly once each"
else
  bad "resuming at ${resume:-?}: HTTP $second_call, $second_result; the calls named '$together', the log has '$want_seqs'"
fi

finish
