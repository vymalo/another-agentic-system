#!/usr/bin/env sh
# System-level test of an agent calling a person's own MCP server (ADR 0046, slice 5): a person who holds `tools.user.add` adds a server of
# their own (a bearer token), attaches it to a conversation, the chat's model calls it through the thread's endpoint, and the orchestrator
# relays the call with a credential it leases for that one call, through a client that judges every connection. The agent is `chat`
# (adam-agent, a folder, on the scripted `mock-model`), the server is `mock-mcp-search`, which the stack names in
# `userToolServers.egress` of dev/orchestrator.yaml. This is the half of dev/tools-e2e.sh (the deployment's `websearch`) and of
# dev/user-tools-e2e.sh (the routes, with nothing connected to) that neither can show.
#
#   dev/user-tools-relay-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; `chat` runs from the same image):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the web does (docs/api/agui.md): one POST /agui/agents/{agentId} per message, the thread from
# GET /api/threads/{id}, its frames from GET /agui/threads/{id}/connect?mode=run, its log from GET /api/threads/{id}/export. The model of
# `chat` is `mock-persona` (dev/wiremock/model): a message that carries `[mock:own-search]` selects the script, which calls the tool whose
# name starts with `u-` and ends with `__web_search` in the list of tools it was offered (the name has a random id, so the script reads it
# from the request with WireMock's response templating), answers from the result, and says that no search server of the person's own is
# offered when there is none in the list.
#
# THE CREDENTIAL. The person's token is made up for the run and starts with `user-token-`, the prefix `mock-mcp-search` is given
# (`MOCK_MCP_TOKEN_PREFIX` in compose.yaml), so the stack holds only the prefix and nothing that is the token. The mock does not keep a token; it
# journals the SHA-256 of the one a call carried and the `Host` it named (dev/mock-mcp-search), which is how the script shows that the call
# carried exactly that token and which name it went to. The run makes up four secrets, each unique to it (the token, the credential of the server
# the guard refuses, a wrong key, and the key that replaces it): none of them may be found in anything the system says.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the person (`tool-maker@example.com`, the role `tool-maker`) adds "My search", a `bearer` server at the mock: 201, `connected`, `ready`,
#     theirs, holding a credential, the token in no answer; GET /api/tool-servers lists it (`own`, `ready`) and shows no URL, kind, state,
#     revision or secret of it;
#   * THE CALL. A thread created with `forwardedProps["vymalo.tools"] = ["<u-id>"]` and "[mock:own-search] Who won the football world cup in
#     2014?": the run ends with RUN_FINISHED, the thread ends `done` and says it has the server attached, the model was offered
#     `<u-id>__web_search` (and not the deployment's `websearch__web_search`) and got the result back, and the answer cites the first link of the
#     mock; the log has one `tools_attached` of the id and EXACTLY ONE tool step, two events of one id, `running` then `completed`, with
#     `icon: mcp-server:<u-id>`, the label `My search · web_search`, the query as its input and the mock's links as its output; the frames say
#     the same (the `vymalo.tools` card and one `vymalo.step`); the mock was called once, for `web_search` with that query, on the name
#     `mock-mcp-search:8080`, carrying THE TOKEN (its SHA-256 is the token's) and none of the deployment's header (`X-Search-Tenant` is the
#     `websearch` server's, not a stranger's), after a handshake and a listing that the mock's request journal shows;
#   * ANOTHER PERSON'S ID. `tool-maker-2` naming that id in a run is 422, nothing is created, and the mock was called no more;
#   * REVOCATION. The same person with a token whose role was taken away (`OIDC_ROLES=user`, the same subject; dev/auth-header.sh): `/api/me`
#     lists no `tools.user.add`; the picker still lists the server, `unavailable`, `not_permitted`; the NEXT MESSAGE on the thread that has it
#     detaches it, in the commit that records the message (a `tools_detached` of the id before the `user_message`), the model is offered nothing of
#     theirs, the mock is not called, and the thread has none attached; the server itself is as it was (connected, its credential kept), and
#     attaching it is 422 `tool_server_unavailable`;
#   * THE PERMISSION BACK. With the role again, `PUT /api/threads/{id}/tools` attaches the server (200, `servers: [<u-id>]`), the next message calls
#     it (a second step, completed, the token again) and the answer cites the link;
#   * DETACH. `PUT /api/threads/{id}/tools` with an empty list: 200, `servers: []`, one `tools_detached`; the next message answers that no search server
#     of their own is offered, the model was not offered the tool, and the mock was not called;
#   * THE GUARD AT THE CONNECTION. A second server at `mock-mcp-search.strangers.test`, an alias of the same container (compose.yaml) that
#     the allow list of dev/orchestrator.yaml admits by a `*.strangers.test` entry, which names a host and never lifts the ban on a private
#     address: the URL is accepted when it is added and the server is `ready` and attachable (the picker judges names; the address is judged at the
#     connection), a thread created with it attached is answered without the tool, and THE MOCK WAS SENT NOTHING under that name, not even the
#     handshake (its journal of every request, by `Host`), nor the credential; the orchestrator's log (docker compose) says the server could not
#     be listed and that the rule was `private-address`, which is the guard and not an unreachable name; the refusal changes no state of the
#     server. A listing that fails is a tool left out, not a failed step: the step with "is not reachable from here" is a call that was made, and a
#     name is judged by the same lists at the listing as at the call, so a model is never offered what a call would be refused (the words of the
#     step are the Rust test `user_relay.rs`'s);
#   * A WRONG KEY. A server at the mock whose token is not one the mock accepts: attached at the creation of a thread, the listing is answered 401,
#     the model is offered nothing, and the definition becomes `needs_reauth` (`GET /api/user-tool-servers`: `needs_auth`, `reauthorize`, the picker
#     says so); the mock saw the request and journals no call; attaching it is then 422 `tool_server_needs_auth`; a new key by PATCH makes it
#     `connected` and `ready` again, it attaches, and the next message calls it with the NEW token (the digest);
#   * NO SECRET. None of the four secrets of the run is in any answer of the API, the export, the frames, the thread, the lists, the journal of the
#     mock, anything the model was sent (its request journal), or the logs of the orchestrator and of the chat agent (docker compose: skipped
#     otherwise);
#   * the servers the run added are deleted (204), and the lists are empty: re-running the script, or running dev/user-tools-e2e.sh, finds nothing.
# A journal the script cannot read is a FAIL, never an empty one (an empty journal would pass every check that something was NOT sent), and every
# "was not offered" check follows one that the model was asked at all.
# Exit status 0 when every check passed. The attach, the lease, the guard and the scrub of what a stranger says are proved by Rust tests
# (orchestrator/crates/surface-thread-tools/tests/user_relay*.rs, crates/tools-mcp/tests/guarded.rs); this is the same chain on the stack.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL              http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   OIDC_URL              http://127.0.0.1:${MOCK_OIDC_PORT:-8099}, the mock issuer (dev/auth-header.sh reads it, and OIDC_CLIENT_ID and _SECRET)
#   MOCK_MODEL_URL        http://127.0.0.1:${MOCK_MODEL_PORT:-8094}
#   MOCK_MCP_SEARCH_URL   http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}, where the script reads the mock's journals
#   MOCK_HOST, MOCK_PORT  mock-mcp-search, 8080: the name and port the ORCHESTRATOR connects to (the compose network's), the one host
#                         `userToolServers.egress.allow` of dev/orchestrator.yaml names exactly
#   STRANGER_HOST         mock-mcp-search.strangers.test: the alias of the same container that only the `*.strangers.test` entry admits
#   USER_TOKEN_PREFIX     user-token-: the prefix the mock accepts as a person's own token (`MOCK_MCP_TOKEN_PREFIX` in compose.yaml)
#   TIMEOUT               120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-model` and the journal of `mock-mcp-search` at the start of each part, so run it on a stack you are not in the
# middle of another scenario on. It deletes the servers of `tool-maker` first (an earlier run that was killed), and again at the end. Needs curl and jq
# (and sha256sum, shasum or openssl); docker compose for the checks of the logs. Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
model=${MOCK_MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
model=${model%/}
search=${MOCK_MCP_SEARCH_URL:-http://127.0.0.1:${MOCK_MCP_SEARCH_PORT:-8096}}
search=${search%/}
host=${MOCK_HOST:-mock-mcp-search}
port=${MOCK_PORT:-8080}
stranger=${STRANGER_HOST:-mock-mcp-search.strangers.test}
prefix=${USER_TOKEN_PREFIX:-user-token-}
timeout=${TIMEOUT:-120}
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
api=/api/user-tool-servers
own_url="http://$host:$port/mcp"
stranger_url="http://$stranger:$port/mcp"

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
note() { echo "skip $1"; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "user tools relay e2e passed"; else echo "user tools relay e2e FAILED"; exit 1; fi
}
# expect WHAT GOT WANT: one line, ok when they are equal.
expect() {
  if [ "$2" = "$3" ]; then ok "$1: $3"; else bad "$1: got '$2', want '$3'"; fi
}

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
: >"$tmp/seen"

# sha256_hex TEXT: the SHA-256 of the text, in hex, the way the mock journals the token a call carried.
sha256_hex() {
  if command -v sha256sum >/dev/null 2>&1; then printf '%s' "$1" | sha256sum | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then printf '%s' "$1" | shasum -a 256 | cut -d ' ' -f 1
  elif command -v openssl >/dev/null 2>&1; then printf '%s' "$1" | openssl dgst -sha256 -r | cut -d ' ' -f 1
  else echo "sha256sum, shasum or openssl is required and none was found" >&2; exit 2
  fi
}

# --- who, and the secrets of this run ----------------------------------------------------------------------------------
maker="tool-maker@example.com"
maker2="tool-maker-2@example.com"
no_token() { bad "a token for $1 from the mock issuer (is mock-oidc up? OIDC_URL=${OIDC_URL:-http://127.0.0.1:${MOCK_OIDC_PORT:-8099}})"; finish; }
h_maker=$(sh "$here/auth-header.sh" "$maker") || no_token "$maker"
h_maker2=$(sh "$here/auth-header.sh" "$maker2") || no_token "$maker2"
# The same person, subject and e-mail, with the role taken away: a token of the issuer that says `roles: [user]`.
h_limited=$(OIDC_ROLES=user sh "$here/auth-header.sh" "$maker") || no_token "$maker with another role"
ok "the mock issuer gave a token to the person, to another tool-maker, and to the person again with a role that cannot add servers"
auth=$h_maker # the header the helpers below send

rand=$(od -An -N8 -tx1 /dev/urandom 2>/dev/null | tr -d ' \n' || true)
[ -n "$rand" ] || rand=$(date +%N)
stamp="$(date +%s)-$$-$rand"
tok_ok="${prefix}${stamp}-ok"       # the person's token for "My search"
tok_guard="${prefix}${stamp}-guard" # the credential of the server the guard refuses: it must never leave
tok_bad="wrong-key-${stamp}"        # a key the mock does not accept
tok_new="${prefix}${stamp}-rekeyed" # the key that replaces it
digest_ok=$(sha256_hex "$tok_ok")
digest_guard=$(sha256_hex "$tok_guard")
digest_bad=$(sha256_hex "$tok_bad")
digest_new=$(sha256_hex "$tok_new")

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# call HEADER METHOD PATH [JSON]: the status on stdout, the body in $tmp/body (and added to $tmp/seen, everything the API said).
call() {
  if [ $# -ge 4 ]; then
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$2" "$base$3" -H "$1" -H 'content-type: application/json' -d "$4" 2>"$tmp/err" || true
  else
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$2" "$base$3" -H "$1" 2>"$tmp/err" || true
  fi
  cat "$tmp/body" >>"$tmp/seen" 2>/dev/null || true
  echo >>"$tmp/seen"
}
field() { jq -r "$1" "$tmp/body" 2>/dev/null || echo unreadable; }
get() { curl --fail-with-body -sS --max-time 60 -H "$auth" "$base$1"; } # get PATH: with the header in $auth; non-zero when the status is not 2xx
add_body() { # add_body NAME URL AUTH_JSON SECRET
  jq -n --arg name "$1" --arg url "$2" --argjson auth "$3" --arg secret "$4" '{name: $name, url: $url, auth: $auth, secret: $secret}'
}

sse_events() { sed -n 's/^data: *//p' "$1"; }

# run_agui AGENT THREAD TEXT [FORWARDED_PROPS_JSON]: one run (a message) on the thread, to its end, as the person of $auth. Prints the HTTP
# status; the response is kept in $tmp/run.sse.
run_agui() {
  _props=${4:-}
  [ -n "$_props" ] || _props='{}'
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" --argjson props "$_props" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$auth" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
  cat "$tmp/run.sse" >>"$tmp/seen" 2>/dev/null || true
}

run_outcome() { # run_outcome: how the run saved by the last run_agui ended
  sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true
}

# wait_state THREAD STATE...: waits (up to TIMEOUT seconds) until the thread is in one of the states, and prints the one it ended in.
wait_state() {
  _thread=$1
  shift
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(get "/api/threads/$_thread" 2>/dev/null | jq -r '.state // empty' || true)
    for _want in "$@"; do
      if [ "$_state" = "$_want" ]; then echo "$_state"; return 0; fi
    done
    case $_state in failed | cancelled) echo "$_state"; return 0 ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then echo "${_state:-unknown}"; return 0; fi
    sleep 2
  done
}

# unreadable WHAT: something this script reads to prove that a thing did NOT happen cannot be read. An empty stand-in would pass those checks, so it stops the run.
unreadable() {
  bad "$1: $(head -c 200 "$tmp/err" 2>/dev/null)"
  finish
}

# snapshot THREAD NAME: the thread, its export and its frames, saved as $tmp/NAME.thread.json, NAME.export.json and NAME.frames.json
snapshot() {
  get "/api/threads/$1" >"$tmp/$2.thread.json" 2>"$tmp/err" || unreadable "snapshot $2: GET /api/threads/$1 failed"
  get "/api/threads/$1/export" >"$tmp/$2.export.json" 2>"$tmp/err" || unreadable "snapshot $2: GET /api/threads/$1/export failed"
  jq -e '.events | type == "array"' "$tmp/$2.export.json" >/dev/null 2>&1 || unreadable "snapshot $2: the export of $1 has no events list"
  curl -sS --max-time 60 -H "$auth" -H 'accept: text/event-stream' "$base/agui/threads/$1/connect?mode=run" 2>"$tmp/err" |
    sed -n 's/^data: *//p' | jq -s '.' >"$tmp/$2.frames.json" 2>/dev/null || unreadable "snapshot $2: the frames of $1 cannot be read"
  cat "$tmp/$2.thread.json" "$tmp/$2.export.json" "$tmp/$2.frames.json" >>"$tmp/seen"
}

# mark_seq THREAD: sets $since to the last event of the thread's log, so that what comes after is this part's
mark_seq() {
  _export=$(get "/api/threads/$1/export" 2>"$tmp/err") || unreadable "GET /api/threads/$1/export failed"
  since=$(printf '%s' "$_export" | jq -er '[.events[].seq] | max' 2>/dev/null) || unreadable "the export of $1 holds no event"
}

# --- the journals --------------------------------------------------------------------------------------------------------
# reset_journals: both start empty. What each held before is kept for the search for secrets at the end.
reset_journals() {
  keep_journals
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$model/__admin/requests" || true)
  if [ "$_code" != 200 ]; then bad "journal reset: $model answered HTTP $_code"; fi
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$search/__journal" || true)
  if [ "$_code" != 200 ]; then bad "journal reset: $search answered HTTP $_code"; fi
}
keep_journals() {
  # a request no script answers is a failure of this scenario's own part (not of what ran before it: the first call has no part behind it)
  if [ -s "$tmp/journals" ]; then
    _unmatched=$(curl -fsS --max-time 30 "$model/__admin/requests/unmatched" 2>/dev/null | jq -r '.requests | length' 2>/dev/null || echo '?')
    case $_unmatched in
      0) ;;
      '?') bad "the requests no script of mock-model answers cannot be read ($model/__admin/requests/unmatched)" ;;
      *) bad "mock-model got $_unmatched request(s) no script answers" ;;
    esac
  fi
  # what cannot be read cannot be searched for secrets: say so rather than search nothing
  curl -fsS --max-time 30 "$model/__admin/requests" >>"$tmp/journals" 2>/dev/null || bad "the request journal of mock-model cannot be read ($model/__admin/requests): the secrets cannot be looked for in it"
  curl -fsS --max-time 30 "$search/__journal" >>"$tmp/journals" 2>/dev/null || bad "the journal of mock-mcp-search cannot be read ($search/__journal): the secrets cannot be looked for in it"
}
: >"$tmp/journals"

# load_persona: sets $reqs to the bodies of the requests the model mock got for mock-persona since its last reset, the oldest first (WireMock lists the newest first).
# A mock that cannot be read stops the run: an empty list would make every "the model was not offered the tool" true.
load_persona() {
  _raw=$(curl -fsS --max-time 30 "$model/__admin/requests" 2>"$tmp/err") || unreadable "the request journal of mock-model cannot be read ($model/__admin/requests)"
  reqs=$(printf '%s' "$_raw" | jq -c '[.requests | reverse | .[].request.body | fromjson? | select(.model == "mock-persona")]' 2>"$tmp/err") ||
    unreadable "the request journal of mock-model is not a WireMock journal"
}
offered() { # offered REQUESTS INDEX: the names of the functions the model was offered in that request
  printf '%s' "$1" | jq -r --argjson i "$2" '[.[$i].tools // [] | .[].function.name] | join(" ")'
}
# model_asked WHAT: the control of every "was not offered" check, which holds of a request that was never made. At least one request reached the model for WHAT.
model_asked() {
  _n=$(printf '%s' "$reqs" | jq -r 'length' 2>/dev/null || echo '?')
  if [ "$_n" -ge 1 ] 2>/dev/null; then ok "the model was asked ($_n request(s) for $1), so what it was offered is what the checks below read"; else bad "the model got no request for $1 ('$_n'): \"was not offered\" would be true of anything"; fi
}
# mock_journal: reads the journals of mock-mcp-search into $tmp/mock.json. A mock that cannot be read stops the run: an empty journal would pass every check that something
# was NOT sent to it.
mock_journal() {
  curl -fsS --max-time 30 "$search/__journal" >"$tmp/mock.json" 2>"$tmp/err" || unreadable "the journal of mock-mcp-search cannot be read ($search/__journal)"
  jq -e '(.calls | type == "array") and (.requests | type == "array")' "$tmp/mock.json" >/dev/null 2>&1 ||
    unreadable "the journal of mock-mcp-search has no calls and requests lists"
}
# on_name HOST: how many requests of any kind (handshake included) reached the mock's /mcp under that Host, and how many tool calls
on_name() {
  jq -r --arg h "$1" '"\([.requests[] | select(.host == $h)] | length) \([.calls[] | select(.host == $h)] | length)"' "$tmp/mock.json"
}

# Every tool step of an export, in order, after `$since`; the steps of the agent's own work are not tool steps.
# shellcheck disable=SC2016 # jq's own variables, not the shell's
tool_steps='[.events[] | select(.seq > $since and .kind == "agent_step" and .data.kind == "tool")]'
answer_of() { # answer_of EXPORT SINCE: the final words of the agent after that seq
  jq -r --argjson since "$2" '[.events[] | select(.seq > $since and .kind == "agent_message" and .data.final == true) | .data.text] | last // empty' "$1"
}

# --- the stack, and what the scenario needs ---------------------------------------------------------------------------------
agents=$(get /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" chat "*) ;;
  *) echo "the agent 'chat' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac
expect "GET /api/me as the person" "$(call "$h_maker" GET /api/me)" 200
case ",$(jq -r '[.permissions[].permission] | join(",")' "$tmp/body")," in
  *,tools.user.add,*) ok "the person holds tools.user.add" ;;
  *) bad "the person lacks tools.user.add: is dev/orchestrator.yaml's role tool-maker there?"; finish ;;
esac

# Whatever an earlier run left of this person's.
clean() {
  _status=$(call "$h_maker" GET "$api")
  [ "$_status" = 200 ] || return 0
  for _id in $(jq -r '.servers[].id' "$tmp/body" 2>/dev/null); do
    call "$h_maker" DELETE "$api/$_id" >/dev/null
  done
}
clean

# --- the person adds a server of their own ---------------------------------------------------------------------------------------
echo "== the person adds \"My search\" at $own_url"
expect "POST a server with a bearer token" "$(call "$h_maker" POST "$api" "$(add_body "My search" "$own_url" '{"kind":"bearer"}' "$tok_ok")")" 201
expect "  it is connected and ready, theirs, and holds a credential" \
  "$(field '[.state, .status, .own, .hasCredential, .auth.kind] | map(tostring) | join(",")')" "connected,ready,true,true,bearer"
own_id=$(field .id)
case $own_id in u-??????????) ok "  its id is u- and ten characters: $own_id" ;; *) bad "  its id is '$own_id'" ;; esac
expect "GET /api/tool-servers" "$(call "$h_maker" GET /api/tool-servers)" 200
expect "  lists it, named, theirs and ready" \
  "$(field '[.[] | select(.id == "'"$own_id"'") | [.name, (.own | tostring), .status] | join(",")] | join(" ")')" "My search,true,ready"
expect "  and shows no URL, kind, state, revision or secret of it" \
  "$(field '[.[] | select(.id == "'"$own_id"'") | has("url") or has("auth") or has("state") or has("revision") or has("secret")] | any | tostring')" false
case $(cat "$tmp/body") in
  *"$host"*) bad "  the list names the server's address ($host)" ;;
  *) ok "  the list does not name the server's address" ;;
esac

# --- the call --------------------------------------------------------------------------------------------------------------------
reset_journals
thread=$(uuid)
echo "== the call: thread $thread (chat), the server attached by the run that creates it"
code=$(run_agui chat "$thread" "[mock:own-search] Who won the football world cup in 2014?" "$(jq -nc --arg id "$own_id" '{"vymalo.tools": [$id]}')")
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/chat answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
expect "the run stream ended with RUN_FINISHED (success)" "$(run_outcome)" "success"
expect "the thread ended done" "$(wait_state "$thread" "done" blocked)" "done"
snapshot "$thread" one
expect "the thread says it has the server attached" "$(jq -r '(.tools // []) | join(",")' "$tmp/one.thread.json")" "$own_id"
answer=$(answer_of "$tmp/one.export.json" 0)
echo "the chat said: ${answer:-<nothing>}"
case $answer in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the answer cites the first link of the mock search" ;;
  *) bad "the answer does not cite https://example.org/mock-search/world-cup-2014 (the tool was not offered or not called, or its result did not go back to the model)" ;;
esac
case $answer in
  "No search server of your own"*) bad "the chat says no search server of the person's own is offered: the server was not listed on the thread's endpoint" ;;
  *) ok "the chat did not say that none is offered" ;;
esac

since=0
expect "the log has one tools_attached, of the id, by the person" \
  "$(jq -r '[.events[] | select(.kind == "tools_attached") | "\(.data.servers | join(",")) \(.actor.type)"] | join(" | ")' "$tmp/one.export.json")" "$own_id user"
expect "exactly one tool step (one id, two events)" \
  "$(jq -r --argjson since "$since" "$tool_steps"' | [(map(.data.id) | unique | length), length] | join(" ")' "$tmp/one.export.json")" "1 2"
expect "the step runs, then completes" \
  "$(jq -r --argjson since "$since" "$tool_steps"' | [.[] | "\(.data.phase):\(.data.state)"] | join(" ")' "$tmp/one.export.json")" "start:running end:completed"
expect "both events carry the icon mcp-server:<the id>" \
  "$(jq -r --argjson since "$since" "$tool_steps"' | map(.data.icon) | unique | join(" ")' "$tmp/one.export.json")" "mcp-server:$own_id"
expect "the label is the server's name the person typed and the tool's" \
  "$(jq -r --argjson since "$since" "$tool_steps"' | map(.data.label) | unique | join(" ")' "$tmp/one.export.json")" "My search · web_search"
step_query=$(jq -r --argjson since "$since" "$tool_steps"' | first | .data.input.query // empty' "$tmp/one.export.json")
case $step_query in
  *"world cup"*) ok "the step's start carries the call's input, the query \"$step_query\"" ;;
  *) bad "the step's start carries no input with the query (input.query: '${step_query:-none}')" ;;
esac
case $(jq -r --argjson since "$since" "$tool_steps"' | last | .data.output.text // empty' "$tmp/one.export.json") in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the step's end carries the output, the links of the mock search" ;;
  *) bad "the step's end carries no output with a link of the mock search" ;;
esac
expect "the step is not an error" "$(jq -r --argjson since "$since" "$tool_steps"' | last | .data.output.error // false' "$tmp/one.export.json")" "false"
expect "a vymalo.tools card says the server was attached" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.tools") | .content.attached // [] | join(",")] | join(" ")' "$tmp/one.frames.json")" "$own_id"
expect "one tool step in the frames, with the icon, running and then completed" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step" and .content.kind == "tool")]
    | [(map(.content.id) | unique | length), (map(.content.icon) | unique | join(",")), (map(.content.state) | join(">"))] | join(" ")' "$tmp/one.frames.json")" \
  "1 mcp-server:$own_id running>completed"

echo "== what the mock was sent"
mock_journal
expect "one call of the tool" "$(jq -r '.calls | length' "$tmp/mock.json")" "1"
expect "  web_search, with the query of the step" "$(jq -r '.calls[0] | "\(.tool) \(.arguments.query)"' "$tmp/mock.json")" "web_search $step_query"
expect "  on the name the person's URL has" "$(jq -r '.calls[0].host' "$tmp/mock.json")" "$host:$port"
expect "  carrying the person's token (the SHA-256 of the one it carried is the token's)" "$(jq -r '.calls[0].token_sha256' "$tmp/mock.json")" "$digest_ok"
expect "  and not the header of the deployment's websearch server" "$(jq -r '.calls[0].headers["x-search-tenant"] // "absent"' "$tmp/mock.json")" "absent"
# a set difference, not `contains`: on strings jq's contains is a substring test, and `notifications/initialized` holds `initialize`
expect "initialize, tools/list and tools/call all reached it under that name too (the journal sees names), by exact method" \
  "$(jq -r --arg h "$host:$port" '(["initialize", "tools/list", "tools/call"] - [.requests[] | select(.host == $h) | .rpc]) | if length == 0 then "none missing" else join(",") end' "$tmp/mock.json")" "none missing"

echo "== what the model was sent"
load_persona
expect "two model requests, the call and then the answer" "$(printf '%s' "$reqs" | jq -r 'length')" "2"
names=$(offered "$reqs" 0)
case " $names " in
  *" ${own_id}__web_search "*) ok "the model was offered ${own_id}__web_search, the relayed tool (tools: $names)" ;;
  *) bad "the model was offered '${names:-no tool}', want ${own_id}__web_search among them" ;;
esac
case " $names " in
  *" websearch__web_search "*) bad "the model was offered the deployment's websearch__web_search, which this thread does not have attached" ;;
  *) ok "and not the deployment's websearch__web_search" ;;
esac
case $(printf '%s' "$reqs" | jq -r '[.[1].messages // [] | .[] | select(.role == "tool") | .content] | last // empty') in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the result of the search went back to the model" ;;
  *) bad "the second model request holds no search result" ;;
esac

# --- another person's id ------------------------------------------------------------------------------------------------------
echo "== another person names the id"
auth=$h_maker2
foreign=$(uuid)
code=$(run_agui chat "$foreign" "[mock:own-search] hello" "$(jq -nc --arg id "$own_id" '{"vymalo.tools": [$id]}')")
expect "tool-maker-2 attaching it in a run is 422 (an id nobody of theirs has, as one that never existed)" "$code" 422
auth=$h_maker
expect "  and creates no thread" "$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -H "$h_maker2" "$base/api/threads/$foreign")" 404
mock_journal
expect "  and the mock was called no more" "$(jq -r '.calls | length' "$tmp/mock.json")" "1"

# --- revocation ---------------------------------------------------------------------------------------------------------------
echo "== revocation: the same person, with a role that cannot add servers"
expect "GET /api/me with that token" "$(call "$h_limited" GET /api/me)" 200
case ",$(jq -r '[.permissions[].permission] | join(",")' "$tmp/body")," in
  *,tools.user.add,*) bad "the token with the role taken away still holds tools.user.add (does dev/mock-oidc take OIDC_ROLES?)"; finish ;;
  *) ok "it does not hold tools.user.add" ;;
esac
expect "GET /api/tool-servers with it" "$(call "$h_limited" GET /api/tool-servers)" 200
expect "  still lists the server, unavailable and not permitted" \
  "$(field '[.[] | select(.id == "'"$own_id"'") | [.status, .reason] | join(",")] | join(" ")')" "unavailable,not_permitted"
mark_seq "$thread"
reset_journals
auth=$h_limited
code=$(run_agui chat "$thread" "[mock:own-search] And now?")
expect "a message on the thread that has it is accepted" "$code" 200
expect "the thread ended done" "$(wait_state "$thread" "done" blocked)" "done"
snapshot "$thread" two
auth=$h_maker
expect "the thread has none attached" "$(jq -r '(.tools // []) | length' "$tmp/two.thread.json")" 0
expect "one tools_detached, of the id, and before the message it came with" \
  "$(jq -r --argjson since "$since" '[.events[] | select(.seq > $since)] as $new
    | ([$new[] | select(.kind == "tools_detached") | .seq] | first) as $d | ([$new[] | select(.kind == "user_message") | .seq] | first) as $m
    | [([$new[] | select(.kind == "tools_detached") | .data.servers | join(",")] | join(" ")), (($d != null and $m != null and $d < $m) | tostring)] | join(" ")' "$tmp/two.export.json")" \
  "$own_id true"
answer=$(answer_of "$tmp/two.export.json" "$since")
case $answer in
  "No search server of your own"*) ok "the answer says that no search server of their own is offered" ;;
  *) bad "the answer is not \"No search server of your own...\": '${answer:-nothing}'" ;;
esac
load_persona
model_asked "the message after the permission was taken away"
names=$(offered "$reqs" 0)
case " $names " in
  *" ${own_id}__web_search "*) bad "the model was offered the person's tool after the permission was taken away (tools: $names)" ;;
  *) ok "the model was not offered the person's tool (tools: ${names:-none})" ;;
esac
mock_journal
expect "the mock was called no more" "$(jq -r '.calls | length' "$tmp/mock.json")" "0"
expect "GET $api with it: the server is as it was (connected)" "$(call "$h_limited" GET "$api")" 200
expect "  connected, with its credential, and not usable for them (unavailable, not_permitted)" \
  "$(field '[.servers[] | select(.id == "'"$own_id"'") | [.state, (.hasCredential | tostring), .status, .reason] | join(",")] | join(" ")')" "connected,true,unavailable,not_permitted"
expect "attaching it is refused to them: PUT /api/threads/{id}/tools" "$(call "$h_limited" PUT "/api/threads/$thread/tools" "$(jq -nc --arg id "$own_id" '{servers: [$id]}')")" 422
expect "  with the code tool_server_unavailable" "$(field .code)" tool_server_unavailable

# --- the permission back ----------------------------------------------------------------------------------------------------------
echo "== the permission back: attach by PUT, and the next message calls the server"
expect "PUT /api/threads/{id}/tools with the id" "$(call "$h_maker" PUT "/api/threads/$thread/tools" "$(jq -nc --arg id "$own_id" '{servers: [$id]}')")" 200
expect "  the set after the change" "$(field '.servers | join(",")')" "$own_id"
mark_seq "$thread"
reset_journals
code=$(run_agui chat "$thread" "[mock:own-search] Who won the football world cup in 2014? Again.")
expect "the message is accepted" "$code" 200
expect "the thread ended done" "$(wait_state "$thread" "done" blocked)" "done"
snapshot "$thread" three
case $(answer_of "$tmp/three.export.json" "$since") in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the answer cites the first link of the mock search" ;;
  *) bad "the answer does not cite the link: '$(answer_of "$tmp/three.export.json" "$since")'" ;;
esac
expect "one new tool step, running then completed, with the icon" \
  "$(jq -r --argjson since "$since" "$tool_steps"' | [(map(.data.id) | unique | length), ([.[] | "\(.data.phase):\(.data.state)"] | join(">")), (map(.data.icon) | unique | join(","))] | join(" ")' "$tmp/three.export.json")" \
  "1 start:running>end:completed mcp-server:$own_id"
mock_journal
expect "the mock was called once, with the person's token" "$(jq -r '[(.calls | length), .calls[0].token_sha256] | join(" ")' "$tmp/mock.json")" "1 $digest_ok"

# --- detach -------------------------------------------------------------------------------------------------------------------------
echo "== detach: PUT /api/threads/$thread/tools with an empty list"
mark_seq "$thread"
expect "PUT with an empty list" "$(call "$h_maker" PUT "/api/threads/$thread/tools" '{"servers":[]}')" 200
expect "  the set after the change is empty" "$(field '.servers | length')" 0
reset_journals
code=$(run_agui chat "$thread" "[mock:own-search] And now?")
expect "the next message is accepted" "$code" 200
expect "the thread ended done" "$(wait_state "$thread" "done" blocked)" "done"
snapshot "$thread" four
expect "the log has one tools_detached, of the id" \
  "$(jq -r --argjson since "$since" '[.events[] | select(.seq > $since and .kind == "tools_detached") | .data.servers | join(",")] | join(" ")' "$tmp/four.export.json")" "$own_id"
case $(answer_of "$tmp/four.export.json" "$since") in
  "No search server of your own"*) ok "the answer says that no search server of their own is offered" ;;
  *) bad "the answer to the message after the detach is not \"No search server of your own...\"" ;;
esac
load_persona
model_asked "the message after the detach"
case " $(offered "$reqs" 0) " in
  *" ${own_id}__web_search "*) bad "the model was offered the tool after the detach" ;;
  *) ok "the model was not offered the tool after the detach" ;;
esac
mock_journal
expect "the mock was called no more" "$(jq -r '.calls | length' "$tmp/mock.json")" "0"

# --- the guard at the connection -----------------------------------------------------------------------------------------------------------
echo "== the guard: a name admitted by a *. entry that resolves into the cluster ($stranger)"
expect "POST a server at $stranger_url" "$(call "$h_maker" POST "$api" "$(add_body "Stranger's search" "$stranger_url" '{"kind":"bearer"}' "$tok_guard")")" 201
expect "  the URL is accepted: connected and ready (a name is judged by the lists, an address at the connection)" \
  "$(field '[.state, .status] | join(",")')" "connected,ready"
guard_id=$(field .id)
expect "the picker says ready too" "$(call "$h_maker" GET /api/tool-servers)" 200
expect "  ready" "$(field '[.[] | select(.id == "'"$guard_id"'") | .status] | join(" ")')" "ready"
reset_journals
gthread=$(uuid)
code=$(run_agui chat "$gthread" "[mock:own-search] Who won the football world cup in 2014?" "$(jq -nc --arg id "$guard_id" '{"vymalo.tools": [$id]}')")
expect "a thread created with it attached is accepted (it is ready)" "$code" 200
expect "the thread ended done" "$(wait_state "$gthread" "done" blocked)" "done"
snapshot "$gthread" guard
expect "the thread says it has the server attached" "$(jq -r '(.tools // []) | join(",")' "$tmp/guard.thread.json")" "$guard_id"
case $(answer_of "$tmp/guard.export.json" 0) in
  "No search server of your own"*) ok "the chat answers without the tool: it was not listed, and a model is never offered what a call would be refused" ;;
  *) bad "the answer is not \"No search server of your own...\": '$(answer_of "$tmp/guard.export.json" 0)'" ;;
esac
load_persona
model_asked "the message on the thread with the stranger's server"
case " $(offered "$reqs" 0) " in
  *" ${guard_id}__web_search "*) bad "the model was offered ${guard_id}__web_search: the guard did not refuse the listing" ;;
  *) ok "the model was not offered the stranger's tool" ;;
esac
expect "no tool step in the log (nothing was called)" "$(jq -r --argjson since 0 "$tool_steps"' | length' "$tmp/guard.export.json")" 0
mock_journal
expect "THE MOCK WAS SENT NOTHING under $stranger: no request of any kind, no tool call" "$(on_name "$stranger:$port")" "0 0"
expect "  and nothing the mock was sent carried the credential of that server (its SHA-256 is in no request)" \
  "$(jq -r --arg d "$digest_guard" '[.requests[] | select(.token_sha256 == $d)] | length' "$tmp/mock.json")" 0
call "$h_maker" GET "$api" >/dev/null
expect "the refusal changed no state: GET $api" "$(field '[.servers[] | select(.id == "'"$guard_id"'") | [.state, .status] | join(",")] | join(" ")')" "connected,ready"

# --- a wrong key --------------------------------------------------------------------------------------------------------------------------
echo "== a wrong key: the mock answers 401"
expect "POST a server at $own_url whose token the mock does not accept" "$(call "$h_maker" POST "$api" "$(add_body "Old key" "$own_url" '{"kind":"bearer"}' "$tok_bad")")" 201
bad_id=$(field .id)
reset_journals
wthread=$(uuid)
code=$(run_agui chat "$wthread" "[mock:own-search] Who won the football world cup in 2014?" "$(jq -nc --arg id "$bad_id" '{"vymalo.tools": [$id]}')")
expect "a thread created with it attached is accepted (nothing has connected yet)" "$code" 200
expect "the thread ended done" "$(wait_state "$wthread" "done" blocked)" "done"
snapshot "$wthread" wrong
load_persona
model_asked "the message on the thread with the server whose key is wrong"
case " $(offered "$reqs" 0) " in
  *" ${bad_id}__web_search "*) bad "the model was offered the tool of a server that answered 401" ;;
  *) ok "the model was not offered the tool of a server that answered 401" ;;
esac
# the control, in the scripted model's own words: when no search server of the person's own is in the list it was offered, it says so
case $(answer_of "$tmp/wrong.export.json" 0) in
  "No search server of your own"*) ok "the chat answers that no search server of its own is offered: the listing of the wrong key left the tool out" ;;
  *) bad "the answer is not \"No search server of your own...\" (the script's words when the tool is not offered): '$(answer_of "$tmp/wrong.export.json" 0)'" ;;
esac
mock_journal
expect "the mock was sent the wrong key (a request that carried it) and journals no call" \
  "$(jq -r --arg d "$digest_bad" '[([.requests[] | select(.token_sha256 == $d)] | length > 0), (.calls | length)] | map(tostring) | join(" ")' "$tmp/mock.json")" "true 0"
expect "GET $api: the definition needs the person to sign in again" "$(call "$h_maker" GET "$api")" 200
expect "  needs_reauth, needs_auth, reauthorize" \
  "$(field '[.servers[] | select(.id == "'"$bad_id"'") | [.state, .status, .reason] | join(",")] | join(" ")')" "needs_reauth,needs_auth,reauthorize"
expect "the picker says so too" "$(call "$h_maker" GET /api/tool-servers)" 200
expect "  needs_auth, reauthorize" "$(field '[.[] | select(.id == "'"$bad_id"'") | [.status, .reason] | join(",")] | join(" ")')" "needs_auth,reauthorize"
expect "attaching it is refused: PUT /api/threads/{id}/tools" "$(call "$h_maker" PUT "/api/threads/$thread/tools" "$(jq -nc --arg id "$bad_id" '{servers: [$id]}')")" 422
expect "  with the code tool_server_needs_auth" "$(field .code)" tool_server_needs_auth
expect "PATCH a new key" "$(call "$h_maker" PATCH "$api/$bad_id" "$(jq -nc --arg s "$tok_new" '{secret: $s}')")" 200
expect "  the server is connected and ready again" "$(field '[.state, .status, .hasCredential] | map(tostring) | join(",")')" "connected,ready,true"
expect "PUT /api/threads/{id}/tools with it now" "$(call "$h_maker" PUT "/api/threads/$thread/tools" "$(jq -nc --arg id "$bad_id" '{servers: [$id]}')")" 200
mark_seq "$thread"
reset_journals
code=$(run_agui chat "$thread" "[mock:own-search] Who won the football world cup in 2014? With the new key.")
expect "the message is accepted" "$code" 200
expect "the thread ended done" "$(wait_state "$thread" "done" blocked)" "done"
snapshot "$thread" five
case $(answer_of "$tmp/five.export.json" "$since") in
  *"https://example.org/mock-search/world-cup-2014"*) ok "the answer cites the first link of the mock search" ;;
  *) bad "the answer does not cite the link: '$(answer_of "$tmp/five.export.json" "$since")'" ;;
esac
mock_journal
expect "the mock was called once, with the NEW token" "$(jq -r '[(.calls | length), .calls[0].token_sha256] | join(" ")' "$tmp/mock.json")" "1 $digest_new"
expect "  the step is labelled with the name of that server" \
  "$(jq -r --argjson since "$since" "$tool_steps"' | map(.data.label) | unique | join(" ")' "$tmp/five.export.json")" "Old key · web_search"

# --- no secret anywhere --------------------------------------------------------------------------------------------------------------------
echo "== no secret anywhere the system keeps or says"
keep_journals
secrets_found() { # secrets_found FILE: the secrets of the run that the file holds (their first characters after the prefix)
  _found=
  for _secret in "$tok_ok" "$tok_guard" "$tok_bad" "$tok_new"; do
    if grep -qF -- "$_secret" "$1"; then _found="$_found $(printf '%s' "$_secret" | cut -c1-14)..."; fi
  done
  echo "$_found"
}
no_secret() { # no_secret WHAT FILE
  _found=$(secrets_found "$2")
  if [ -z "$_found" ]; then ok "no secret of the run in $1"; else bad "a secret of the run is in $1 (starts:$_found)"; fi
}
call "$h_maker" GET "$api" >/dev/null
call "$h_maker" GET /api/tool-servers >/dev/null
no_secret "any answer of the API, the runs' streams, the exports, the frames and the threads this script read" "$tmp/seen"
no_secret "the request journal of the model (everything it was sent) and the journal of the mock search" "$tmp/journals"
compose() { docker compose -f "$root/compose.yaml" --profile app "$@"; }
if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1 && compose ps -q postgres 2>/dev/null | grep -q .; then
  compose logs --no-color orchestrator >"$tmp/logs.orchestrator" 2>&1 || true
  compose logs --no-color chat >"$tmp/logs.chat" 2>&1 || true
  no_secret "the orchestrator's log" "$tmp/logs.orchestrator"
  no_secret "the chat agent's log" "$tmp/logs.chat"
  if grep -F "$guard_id" "$tmp/logs.orchestrator" | grep -F "could not be listed" | grep -qF "private-address"; then
    ok "the orchestrator's log says the stranger's server could not be listed, and that the rule was private-address (the guard, not an unreachable name)"
  else
    bad "the orchestrator's log has no line for $guard_id 'could not be listed' with the rule private-address (did the name resolve? is the entry *.strangers.test in dev/orchestrator.yaml?)"
  fi
else
  note "the logs of the orchestrator and of the chat agent, and why the guard refused: docker compose is not available here, or the stack is not its project; the rest was checked all the same"
fi

# --- cleaning up ----------------------------------------------------------------------------------------------------------------------------
echo "== cleaning up"
for id in "$own_id" "$guard_id" "$bad_id"; do
  expect "DELETE $id" "$(call "$h_maker" DELETE "$api/$id")" 204
done
clean
expect "GET $api after" "$(call "$h_maker" GET "$api")" 200
expect "  nothing is left of the person's" "$(field '.servers | length')" 0

finish
