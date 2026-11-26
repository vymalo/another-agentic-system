#!/usr/bin/env sh
# System-level test of a person's own agent on the message path (ANR-806, part 2): a person's first message to the coder makes
# their agent on the platform, a message to a suspended agent resumes it, both wait until the agent is ready before the message is
# written and sent, and a wait that runs out ends at the configured bound with a 503 and writes nothing.
#
#   dev/provisioner-e2e.sh
#
# Start the stack WITH the override first (the platform is `mock-platform`, a WireMock; the orchestrator has a `provisioner` section):
#
#   docker compose -f compose.yaml -f dev/compose.provisioner.yaml --profile app up -d --build --wait
#
# The platform's owner routes are `/v1/owners/{owner}/agents/{name}` (another-agentic-platform's Platform API). The owner of a person is
# 24 hex digits of the SHA-256 of their user id (`Provisioning::owner_of` in crates/app/src/provisioning.rs) and the agent is `coder-<owner>`.
# The script gives `mock-platform` one state machine (a WireMock scenario) per person through its admin API, then asks the orchestrator
# through the edge, as the UI does (an AG-UI run to the agent `adam`, the coder, whose alias is `coder`: the kind is read from the alias):
#
#   dev@example.com     the first message: the agent is absent (404), `PUT If-None-Match: *` makes it (201, saving), one poll later it is ready
#   holder@example.com  a suspended agent: `GET` says suspended with an ETag, `PUT If-Match: <etag>` with `suspend: false` resumes it, it is ready
#                       a poll later; the scenarios are reset and a second message to the same thread resumes it again
#   admin@example.com   an agent that never gets ready: the run ends at `provisioner.readyTimeoutSecs` of dev/orchestrator.provisioner.yaml
#                       (6 s) with a 503, `code: agent_not_ready`, "not ready after 6s", and no thread
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   (a) a first message makes the agent: the platform saw one `PUT If-None-Match: *` for `coder-<owner>` whose body names the agent, the
#       owner, `suspend: false` and the deployment's form of the coder, with a bearer token; it polled until ready; the thread ends
#       `blocked` or `done` (the coder answered); the thread's first event is later than the last poll of the platform (the message was written
#       after the agent was ready); and a message to `chat`, an agent that is not provisioned, makes no request to the platform;
#   (b) a suspended agent is resumed: one `PUT If-Match: "s1"` with `suspend: false`, then polls until ready, the thread ends `blocked` or
#       `done`; after the scenarios are reset (the agent is suspended again) a second message to the same thread does it again (two resumes);
#   (c) the wait ends at the bound: the run is a 503 `agent_not_ready` after at least 5 s and at most 16 s (the bound, 6 s, and its backstop), the
#       detail says "not ready after 6s", it carries `Retry-After`, the platform saw the create and several polls, and no thread was written.
# The mock platform is left empty (reset). Exit status 0 when every check passed. It leaves two threads behind (dev's and the holder's).
#
# Environment (defaults match compose.yaml and dev/compose.provisioner.yaml on one machine):
#   BASE_URL            http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   MOCK_PLATFORM_URL   http://127.0.0.1:${MOCK_PLATFORM_PORT:-8105}, the platform's WireMock (its admin API)
#   AGENT_ID            adam   the coder (its alias `coder` is what the provisioner's kind is read from)
#   READY_TIMEOUT       6      the `readyTimeoutSecs` of dev/orchestrator.provisioner.yaml
#   TIMEOUT             120    seconds to wait for a thread to stop
#
# It RESETS the stubs and the request journal of `mock-platform` first, so run it on a stack you are not in the middle of another scenario
# on. Needs curl, jq and sha256sum (and /proc or uuidgen for a UUID). Verified by CI only: the step "A person's own agent" of
# .github/workflows/coder-e2e.yml runs it. `sh -n` and shellcheck are what was run where it was written, with no Docker.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
platform=${MOCK_PLATFORM_URL:-http://127.0.0.1:${MOCK_PLATFORM_PORT:-8105}}
platform=${platform%/}
agent=${AGENT_ID:-adam}
ready_timeout=${READY_TIMEOUT:-6}
timeout=${TIMEOUT:-120}

dev_email=dev@example.com
holder_email=holder@example.com
admin_email=admin@example.com
# The API wants a bearer token of the mock issuer (ADR 0033): dev/auth-header.sh prints the header line.
dev_header=$(sh "$here/auth-header.sh" "$dev_email")
holder_header=$(sh "$here/auth-header.sh" "$holder_email")
admin_header=$(sh "$here/auth-header.sh" "$admin_email")

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "provisioner e2e passed"; else echo "provisioner e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# The owner key of a person, as the orchestrator derives it.
owner_of() { printf '%s' "$1" | sha256sum | cut -c1-24; }

admin() { # admin METHOD PATH [BODY]: WireMock's admin API
  if [ $# -ge 3 ]; then
    curl -fsS --max-time 30 -X "$1" "$platform/__admin$2" -H 'content-type: application/json' -d "$3"
  else
    curl -fsS --max-time 30 -X "$1" "$platform/__admin$2"
  fi
}

# stub SCENARIO FROM TO METHOD PATH STATUS STATE ETAG [HEADER NAME VALUE]: one mapping of a person's state machine.
# TO is empty to stay in FROM. The answer is the agent in STATE (empty: a 404 problem).
stub() {
  scenario=$1 from=$2 to=$3 method=$4 path=$5 status=$6 state=$7 etag=$8 hname=${9:-} hvalue=${10:-}
  body=$(jq -cn --arg s "$state" 'if $s == "" then {type: "urn:vymalo:agents:problem:not-found", status: 404} else {state: $s} end')
  jq -n --arg scenario "$scenario" --arg from "$from" --arg to "$to" --arg method "$method" --arg path "$path" \
    --argjson status "$status" --arg body "$body" --arg etag "$etag" --arg hname "$hname" --arg hvalue "$hvalue" '
    {scenarioName: $scenario, requiredScenarioState: $from,
     request: ({method: $method, urlPath: $path} + (if $hname == "" then {} else {headers: {($hname): {equalTo: $hvalue}}} end)),
     response: {status: $status, body: $body,
                headers: ({"Content-Type": (if $status == 404 then "application/problem+json" else "application/json" end)}
                          + (if $etag == "" then {} else {ETag: $etag} end))}}
    + (if $to == "" then {} else {newScenarioState: $to} end)' > "$tmp/stub.json"
  admin POST /mappings "$(cat "$tmp/stub.json")" >/dev/null
}

count() { # count METHOD PATH [HEADER VALUE]: requests the platform saw
  admin POST /requests/count "$(jq -cn --arg m "$1" --arg p "$2" --arg hn "${3:-}" --arg hv "${4:-}" '
    {method: $m, urlPath: $p} + (if $hn == "" then {} else {headers: {($hn): {equalTo: $hv}}} end)')" | jq -r '.count'
}

find_requests() { # find_requests METHOD PATH: the requests, oldest first
  admin POST /requests/find "$(jq -cn --arg m "$1" --arg p "$2" '{method: $m, urlPath: $p}')" |
    jq -c '.requests | sort_by(.loggedDate)'
}

total_requests() { admin GET /requests | jq -r '.requests | length'; }

sse_events() { sed -n 's/^data: *//p' "$1"; }

# run EMAIL_HEADER_FILE THREAD AGENT TEXT: one AG-UI run; writes $tmp/run.sse, $tmp/run.head; prints the HTTP status
run() {
  input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$4" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -D "$tmp/run.head" -w '%{http_code}' -X POST \
    "$base/agui/agents/$3" -H "$(cat "$1")" -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" 2>"$tmp/err" || true
}

api() { # api HEADER_FILE METHOD PATH: the body on stdout; non-zero when the status is not 2xx
  curl --fail-with-body -sS --max-time 60 -X "$2" "$base$3" -H "$(cat "$1")"
}

settle() { # settle HEADER_FILE THREAD: waits for the thread to stop; prints its state
  deadline=$(( $(date +%s) + timeout ))
  while :; do
    st=$(api "$1" GET "/api/threads/$2" 2>/dev/null | jq -r '.state // empty' || true)
    case $st in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
    sleep 2
  done
  printf '%s' "$st"
}

printf '%s' "$dev_header" > "$tmp/dev.h"
printf '%s' "$holder_header" > "$tmp/holder.h"
printf '%s' "$admin_header" > "$tmp/admin.h"

# --- the platform: one state machine per person -----------------------------------------------------------------
if admin GET /health >/dev/null 2>&1; then ok "mock-platform answers at $platform"; else bad "mock-platform does not answer at $platform (the stack of dev/compose.provisioner.yaml?)"; finish; fi
admin POST /mappings/reset >/dev/null
admin DELETE /requests >/dev/null
admin POST /scenarios/reset >/dev/null

dev_owner=$(owner_of "$dev_email");       dev_path=/v1/owners/$dev_owner/agents/coder-$dev_owner
holder_owner=$(owner_of "$holder_email"); holder_path=/v1/owners/$holder_owner/agents/coder-$holder_owner
admin_owner=$(owner_of "$admin_email");   admin_path=/v1/owners/$admin_owner/agents/coder-$admin_owner

# dev: absent, made by the first message, saving, ready one poll later
stub dev Started Saving PUT "$dev_path" 201 saving '"d1"' If-None-Match '*'
stub dev Started "" GET "$dev_path" 404 "" ""
stub dev Saving Ready GET "$dev_path" 200 saving '"d2"'
stub dev Ready "" GET "$dev_path" 200 ready '"d3"'
# holder: suspended, resumed by `PUT If-Match`, saving, ready one poll later
stub holder Started Saving PUT "$holder_path" 200 saving '"s2"' If-Match '"s1"'
stub holder Started "" GET "$holder_path" 200 suspended '"s1"'
stub holder Saving Ready GET "$holder_path" 200 saving '"s3"'
stub holder Ready "" GET "$holder_path" 200 ready '"s4"'
# admin: made, then saving for ever
stub admin Started Stuck PUT "$admin_path" 201 saving '"a1"' If-None-Match '*'
stub admin Started "" GET "$admin_path" 404 "" ""
stub admin Stuck "" GET "$admin_path" 200 saving '"a2"'
ok "stubs for three people: $(admin GET /mappings | jq -r '.mappings | length') mappings"

# --- (a) a first message makes the agent ------------------------------------------------------------------------
thread=$(uuid)
echo "(a) dev's first message, thread $thread"
code=$(run "$tmp/dev.h" "$thread" "$agent" hi)
if [ "$code" = 200 ]; then ok "the run was accepted (HTTP 200)"; else bad "POST /agui/agents/$agent answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"; finish; fi
state=$(settle "$tmp/dev.h" "$thread")
case $state in
  blocked | done) ok "the thread ended $state (the coder answered once the agent was ready)" ;;
  *) bad "the thread ended '${state:-unknown}', want blocked or done" ;;
esac
puts=$(count PUT "$dev_path" If-None-Match '*')
if [ "$puts" = 1 ]; then ok "the platform saw one PUT If-None-Match: * for coder-<owner>"; else bad "the platform saw $puts conditional creates for $dev_path, want 1"; fi
find_requests PUT "$dev_path" > "$tmp/puts.json"
if jq -e --arg owner "$dev_owner" --arg name "coder-$dev_owner" '
    .[0].request.body | fromjson | (.name == $name and .owner == $owner and .suspend == false and .image == "dev/coder:mock" and .model == "mock-model")' \
  "$tmp/puts.json" >/dev/null 2>&1; then
  ok "the create names the agent, the owner, suspend false and the deployment's form of the coder"
else
  bad "the create's body is not the agent's form: $(jq -c '.[0].request.body' "$tmp/puts.json" 2>/dev/null)"
fi
if jq -e '.[0].request.headers | to_entries | any(.key | ascii_downcase == "authorization") and (map(select(.key | ascii_downcase == "authorization"))[0].value | test("^Bearer ."))' \
  "$tmp/puts.json" >/dev/null 2>&1; then
  ok "the platform was asked with a bearer token"
else
  bad "the create carried no bearer token"
fi
if [ "$(admin GET /scenarios | jq -r '.scenarios[] | select(.name == "dev") | .state')" = Ready ]; then
  ok "the orchestrator polled until the platform said ready"
else
  bad "the platform's agent of dev did not reach ready: $(admin GET /scenarios | jq -c '.scenarios[] | select(.name == "dev")')"
fi
# The thread's first event is written after the platform's last answer about the agent: the message waited for ready.
last_poll=$(find_requests GET "$dev_path" | jq -r 'last | .loggedDateString // empty' | cut -c1-19)
api "$tmp/dev.h" GET "/api/threads/$thread/export" > "$tmp/export.json" 2>/dev/null || echo '{"events":[]}' > "$tmp/export.json"
first_event=$(jq -r '.events[0].at // empty' "$tmp/export.json" | cut -c1-19)
if [ -n "$last_poll" ] && [ -n "$first_event" ] && [ "$(date -u -d "${first_event}Z" +%s)" -ge "$(date -u -d "${last_poll}Z" +%s)" ]; then
  ok "the thread's first event ($first_event) is not before the platform's last poll ($last_poll)"
else
  bad "the thread's first event ($first_event) is before the platform's last poll ($last_poll), or one of them is missing"
fi
# An agent that is not provisioned is not asked for.
before=$(total_requests)
chat_thread=$(uuid)
code=$(run "$tmp/dev.h" "$chat_thread" chat hi)
settle "$tmp/dev.h" "$chat_thread" >/dev/null
after=$(total_requests)
if [ "$code" = 200 ] && [ "$before" = "$after" ]; then
  ok "a message to chat made no request to the platform"
else
  bad "a message to chat: HTTP $code, the platform saw $((after - before)) request(s), want none"
fi

# --- (b) a suspended agent is resumed -----------------------------------------------------------------------------
thread_b=$(uuid)
echo "(b) the holder's first message to a suspended agent, thread $thread_b"
code=$(run "$tmp/holder.h" "$thread_b" "$agent" hi)
if [ "$code" = 200 ]; then ok "the run was accepted (HTTP 200)"; else bad "POST /agui/agents/$agent as the holder answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"; finish; fi
state=$(settle "$tmp/holder.h" "$thread_b")
case $state in
  blocked | done) ok "the thread ended $state" ;;
  *) bad "the thread ended '${state:-unknown}', want blocked or done" ;;
esac
resumes=$(count PUT "$holder_path" If-Match '"s1"')
if [ "$resumes" = 1 ]; then ok "the platform saw one PUT If-Match: \"s1\" (the ETag it had just read)"; else bad "the platform saw $resumes resumes of $holder_path, want 1"; fi
if find_requests PUT "$holder_path" | jq -e '.[0].request.body | fromjson | (.suspend == false and .image == "dev/coder:mock")' >/dev/null 2>&1; then
  ok "the resume says suspend: false and carries the form"
else
  bad "the resume's body is not suspend: false and the form"
fi
if [ "$(count PUT "$holder_path" If-None-Match '*')" = 0 ]; then ok "a suspended agent is not made a second time"; else bad "the platform saw a create for a suspended agent"; fi
if [ "$(admin GET /scenarios | jq -r '.scenarios[] | select(.name == "holder") | .state')" = Ready ]; then
  ok "the orchestrator polled until the platform said ready"
else
  bad "the platform's agent of the holder did not reach ready"
fi
# Suspended again: the next message to the same thread resumes it again.
admin POST /scenarios/reset >/dev/null
code=$(run "$tmp/holder.h" "$thread_b" "$agent" "thanks")
state=$(settle "$tmp/holder.h" "$thread_b")
resumes=$(count PUT "$holder_path" If-Match '"s1"')
if [ "$code" = 200 ] && [ "$resumes" = 2 ]; then
  ok "a second message to the same thread resumed the agent again (two resumes, thread $state)"
else
  bad "the second message: HTTP $code, $resumes resumes of $holder_path (want 2), thread '$state'"
fi

# --- (c) the wait ends at the bound ----------------------------------------------------------------------------------
thread_c=$(uuid)
echo "(c) admin's first message to an agent that is never ready, thread $thread_c (the bound is ${ready_timeout}s)"
started=$(date +%s)
code=$(run "$tmp/admin.h" "$thread_c" "$agent" hi)
elapsed=$(( $(date +%s) - started ))
if [ "$code" = 503 ]; then ok "the run was refused with 503"; else bad "the run answered HTTP ${code:-none}, want 503: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"; fi
low=$(( ready_timeout - 1 ))
high=$(( ready_timeout + 10 ))
if [ "$elapsed" -ge "$low" ] && [ "$elapsed" -le "$high" ]; then
  ok "it took ${elapsed}s: the wait-ready bound (${ready_timeout}s), not the dispatch's, and not for ever"
else
  bad "it took ${elapsed}s, want between ${low}s and ${high}s"
fi
if jq -e --arg d "not ready after ${ready_timeout}s" '.code == "agent_not_ready" and (.detail | contains($d))' "$tmp/run.sse" >/dev/null 2>&1; then
  ok "the problem says agent_not_ready and \"not ready after ${ready_timeout}s\""
else
  bad "the problem is not agent_not_ready after ${ready_timeout}s: $(head -c 300 "$tmp/run.sse")"
fi
if grep -qi '^retry-after:' "$tmp/run.head"; then ok "it carries Retry-After"; else bad "no Retry-After"; fi
polls=$(count GET "$admin_path")
if [ "$(count PUT "$admin_path" If-None-Match '*')" = 1 ] && [ "$polls" -ge 3 ]; then
  ok "the platform saw the create and $polls polls"
else
  bad "the platform saw $(count PUT "$admin_path" If-None-Match '*') creates and $polls polls of $admin_path (want 1 and at least 3)"
fi
if api "$tmp/admin.h" GET "/api/threads/$thread_c" >/dev/null 2>&1; then
  bad "a thread was written for the message that was refused"
else
  ok "no thread was written for the refused message"
fi

# --- left as it was found ---------------------------------------------------------------------------------------------
admin POST /mappings/reset >/dev/null
admin DELETE /requests >/dev/null
finish
