#!/usr/bin/env sh
# System-level test of sharing a thread by a revocable link, through the real edge (ADR 0040, sections 6 to 10 and the test plan of
# section 13): the owner shares a thread, another signed-in person and anybody at all read it by the link, none of them can write, the
# deployment's cap narrows every link at once, a revoked link is a 404 and ends the streams open on it, a fork is private, and the edge
# lets through exactly the public link and nothing wider.
#
#   dev/share-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; the `researcher` agent runs from the same image):
#
#   docker compose --profile app up -d --build --wait
#
# The thread is one of `researcher` ("Who won the football world cup in 2014?", dev/agents-e2e.sh): it ends `done` with a tool step, `Web
# search`, that has an input (the query) and an output (the mock's links), which is what a public reader must not be given. The script
# speaks AG-UI to start the run, as the web does (docs/api/agui.md), and the resource API for the rest (docs/api/chat-api.yaml):
# `shareThread`, `rotateThreadShare`, `unshareThread`, `getThread`, `exportThread`, `forkThread`, `getSharedThread`,
# `getPublicSharedThread`, `connectSharedThread`, `connectPublicSharedThread`. Everything goes through the edge (`dev/Caddyfile`, a real
# oauth2-proxy), and the anonymous requests carry no credential at all.
#
# THE CAP. The stack runs `sharing.mode: internal` (dev/orchestrator.yaml). A public link needs `public`, and there is no variable that
# changes one key of the file (ADR 0034), so the script restarts the orchestrator, and only it, on a copy of the stack's own file in
# which that one line says `public` (a temporary compose override that mounts the copy at the same place: no build, and the state is in
# Postgres), then restarts it on the stack's own file again. It is put back whatever happens, also when a check failed or on Ctrl-C (a
# trap). If the script was killed hard and left the cap at `public`, it says so and asks for one command (below). The stack is the offline
# one (`compose.yaml`): a restart with this command would drop the override of `compose.live.yaml` or of the others, so the script is not
# for those stacks. Needs `docker compose` on the machine that runs the stack (it exits 77, a skip, when there is none: dev/e2e-all.sh).
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * AT THE CAP `internal` (the stack as it is):
#       - the owner shares the thread as `internal` (200, a link `/s/<token>` of 43 characters); the log has the `thread_shared` event
#         and the export holds no token (the log never holds a capability); a new link (`rotate`) kills the old one and opens a new one;
#       - another signed-in user (`someone-else@example.com`) reads the link: the view has only the reader's keys, no owner, and not the
#         owner's e-mail; the owner reads their own link as the owner (`isOwner`); the stream (`/agui/shared/<token>/connect`) replays the thread with the person's message sent by "the owner",
#         the step with its input and output (an internal reader has them) and no e-mail anywhere, and ends with an `id:` and no `data:`,
#         the view's `lastSeq` (the share is the log's last event and has no frame: how a reader tells that it has the whole log);
#         a guess and a forged `Authorization` or `X-Auth-Request-Email` are the same 404;
#       - ANY WRITE BY THEM IS REFUSED: the thread is not theirs (404 for share, rotate, revoke, rename, cancel, fork, delete, a message
#         over AG-UI, and for reading it, its export and its stream by its id; not in their list), the link takes no POST, PUT, PATCH or
#         DELETE (405), and the thread is as it was and the link still opens;
#       - the cap: sharing as `public` is 409 `over_cap`, and the link of an `internal` thread is a 404 on the public routes, at the
#         orchestrator and not at the edge (an anonymous request to the signed-in routes is 401 at the edge);
#       - REVOCATION: with a stream open on the link (it has delivered its frames through the edge, unbuffered), `DELETE /share` is 204,
#         the open stream ENDS by itself, the link is a 404 for everybody, the thread says it is private; sharing again makes a new
#         link, and the revoked one never comes back;
#   * THE EDGE, for anonymous requests, with a link that exists: `/` and `/threads/<id>` are sent to sign in, `/api/threads`, `/api/me`,
#     `/agui/threads/<id>/connect`, `/api/shared/<token>` and `/agui/shared/<token>/connect` are 401; a POST to a public path, a path
#     of the public API that is not `shared/`, and a path that climbs out of `shared/` (`../..`, encoded or not) are 401 too: the carve-out
#     is `GET`/`HEAD` of `/api/public/shared/*`, `/agui/public/shared/*` and the web's page and files, and nothing wider. Caddy's path
#     matcher ignores case, so `/API/PUBLIC/SHARED/<token>` is forwarded: the script asserts it is 401 or 404, never the thread. The page
#     `/s/<token>` and the web's files answer with no sign-in (200, or the web's own 404 for a file the build has not), and the page keeps
#     the link out of other sites' referrers (`Referrer-Policy: same-origin`);
#   * AT THE CAP `public` (the orchestrator restarted):
#       - the owner widens the link to `public` and it stays THE SAME LINK; ANONYMOUS `curl`s read it: the view has only the reader's keys,
#         `isOwner` false, no owner, no e-mail, `no-store` and `noindex`; the stream (`/agui/public/shared/<token>/connect`, no sign-in,
#         through the edge) replays the thread with the message sent by "the owner", the step LABEL and NO step input, output or detail, and
#         no e-mail anywhere, where a signed-in reader of the same link has them; files are off (404); the owner's own bearer token on the public
#         route changes nothing (`isOwner` false, no input or output): the identity is ignored, and the edge drops it too (the chart's
#         render check asserts that; here the answer is the same for a bearer that is not a token, which oauth2-proxy would have refused);
#         the page `/s/<token>` and the web's files are served with no sign-in;
#       - A FORK OF THE SHARED THREAD IS PRIVATE: it carries the copy of the share event as history and has no link, its owner's view
#         has no `share`, nobody else can read it, and the parent's link is as it was;
#       - REVOCATION of a public link ends an open anonymous stream, and the link is a 404 for everybody;
#   * WITH THE CAP BACK AT `internal` (the orchestrator restarted on the stack's own file): a link that was made `public` is a 404 on the
#     public routes, at the orchestrator, and is served as `internal` to a signed-in reader (the owner's view says `public` stored and
#     `internal` effective: nothing was rewritten), and the page is still served; the thread is then made private, and the stack is as it was.
# Exit status 0 when every check passed.
#
# What it cannot assert, and where that is asserted instead: that the EDGE strips `Authorization` and `X-Auth-Request-Email` from the
# public routes (a client cannot see a header the orchestrator ignores anyway) is `deploy/chart/tests/render-check.sh` for the chart's
# Caddyfile; the limits of the public routes (429, the stream ceiling) are the Rust tests of `orch-api`; `sharing.public.stepIo` and
# `files: true` are not switched on here. The page in a browser is `web/e2e`, on the web's own mock server.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL      http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL    dev@example.com, the owner: a token of the mock issuer (dev/auth-header.sh)
#   OTHER_EMAIL   someone-else@example.com, the second signed-in user (a user of dev/mock-oidc/users.json)
#   AGENT_ID      researcher, the agent of the thread (it has a tool step with an input and an output)
#   ORCH_CONFIG   dev/orchestrator.yaml, the file the stack mounts: its `sharing.mode: internal` is the one line the copy changes
#   TIMEOUT       120    seconds to wait for a thread to stop, and the longest a stream may stay open
#
# If the cap is left at `public` (the script was killed hard): docker compose --profile app up -d --no-build --no-deps orchestrator
# It leaves one thread (private again) and its fork, like any other scenario. Needs curl and jq (and /proc or uuidgen for a UUID).
# Verified by CI only, in .github/workflows/coder-e2e.yml; the stack could not be run where this script was written: it was dry-run
# against stand-ins behind the real Caddy 2.11.4, oauth2-proxy 7.15.5 and mock issuer, which proves its mechanics and the edge, not the
# orchestrator's answers, so CI is the first run of the rest.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
other_email=${OTHER_EMAIL:-someone-else@example.com}
agent=${AGENT_ID:-researcher}
config=${ORCH_CONFIG:-$root/dev/orchestrator.yaml}
timeout=${TIMEOUT:-120}

# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$here/auth-header.sh" "$email")
other_header=$(sh "$here/auth-header.sh" "$other_email")

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "share e2e passed"; else echo "share e2e FAILED"; exit 1; fi
}

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done
if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
  echo "SKIP  docker compose is not available here: this scenario restarts the orchestrator at two caps." >&2
  exit 77
fi

tmp=$(mktemp -d)
chmod a+rx "$tmp" # the override mounts a file of it into the orchestrator's container, which does not run as this user
background=""
restart_needed=0
thread=""

# compose [ARGS]: the stack's own file; compose_public [ARGS]: the same with the override that mounts the copy with the cap `public`.
compose() { docker compose -f "$root/compose.yaml" --profile app "$@"; }
compose_public() { docker compose -f "$root/compose.yaml" -f "$tmp/compose.public.yaml" --profile app "$@"; }

# Whatever happens: stop the streams, take the link of the thread down, and put the orchestrator back on the stack's own file.
cleanup() {
  status=$?
  trap - EXIT INT TERM
  for _pid in $background; do
    pkill -P "$_pid" 2>/dev/null || true
    kill "$_pid" 2>/dev/null || true
  done
  if [ "$restart_needed" = 1 ]; then
    restart_needed=0
    compose up -d --no-build --no-deps --wait --wait-timeout 180 orchestrator >/dev/null 2>&1 ||
      echo "could not put the orchestrator back on its own configuration: run: docker compose --profile app up -d --no-build --no-deps orchestrator" >&2
  fi
  if [ -n "$thread" ]; then
    curl -sS --max-time 20 -o /dev/null -X DELETE "$base/api/threads/$thread/share" -H "$id_header" 2>/dev/null || true
  fi
  rm -rf "$tmp"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

api() { # api METHOD PATH [BODY]: the body on stdout, non-zero when the status is not 2xx (the resource API, as the owner)
  if [ $# -ge 3 ]; then
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header" -H 'content-type: application/json' -d "$3"
  else
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
  fi
}

call() { # call HEADER METHOD PATH [BODY]: the HTTP status as that user; the body in $tmp/body, the headers in $tmp/headers
  _h=$1 _m=$2 _p=$3
  if [ $# -ge 4 ]; then
    curl -sS --max-time 60 -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' -X "$_m" "$base$_p" -H "$_h" \
      -H 'content-type: application/json' -d "$4"
  else
    curl -sS --max-time 60 -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' -X "$_m" "$base$_p" -H "$_h"
  fi
}

# anon METHOD PATH [CURL ARGUMENTS...]: the HTTP status of a request with NO credential at all; the body in $tmp/body, the headers in
# $tmp/headers. Not followed when it redirects. A 429 (the public routes are limited per link) is asked again, up to four times.
anon() {
  _m=$1 _p=$2
  shift 2
  _try=0
  while :; do
    _code=$(curl -sS --max-time 60 -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' -X "$_m" "$@" "$base$_p" 2>"$tmp/err" || true)
    if [ "$_code" = 429 ] && [ "$_try" -lt 4 ]; then
      _try=$((_try + 1))
      sleep 1
      continue
    fi
    break
  done
  echo "$_code"
}

header() { # header NAME: the value of a response header of the last call or anon
  tr -d '\r' <"$tmp/headers" | awk -v n="$1" '
    BEGIN { n = tolower(n) }
    { i = index($0, ":"); if (i > 0 && tolower(substr($0, 1, i - 1)) == n) { v = substr($0, i + 1); sub(/^ +/, "", v) } }
    END { print v }'
}

state_of() { # state_of THREAD: the state the API says, or nothing
  api GET "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' 2>/dev/null || true
}

wait_state() { # wait_state THREAD STATE: within TIMEOUT seconds the resource API says STATE
  _n=0
  while [ "$(state_of "$1")" != "$2" ]; do
    _n=$((_n + 1))
    [ "$_n" -lt $((timeout * 2)) ] || return 1
    sleep 0.5
  done
}

settle() { # settle THREAD: wait until the thread's last sequence number has not moved for four seconds (a title and a description may be on their way)
  _last=-1 _same=0 _n=0
  while [ "$_same" -lt 4 ]; do
    _now=$(api GET "/api/threads/$1" 2>/dev/null | jq -r '.lastSeq // empty' 2>/dev/null || true)
    if [ -n "$_now" ] && [ "$_now" = "$_last" ]; then _same=$((_same + 1)); else _same=0; _last=$_now; fi
    _n=$((_n + 1))
    [ "$_n" -lt 60 ] || break
    sleep 1
  done
}

# frames FILE: the AG-UI events of a saved SSE response, as one JSON array (a line that is not JSON, a last one that was cut, is left out)
frames() { sed -n 's/^data: *//p' "$1" | jq -Rn '[inputs | fromjson?]' 2>/dev/null || echo '[]'; }

sse_get() { # sse_get PATH OUT [HEADER]: a GET of an event stream, to its end (at most 30 s); prints the HTTP status. No HEADER: no credential
  if [ $# -ge 3 ]; then
    curl -sS -N --max-time 30 -o "$2" -D "$tmp/headers" -w '%{http_code}' -H 'accept: text/event-stream' -H "$3" "$base$1" 2>"$tmp/err" || true
  else
    curl -sS -N --max-time 30 -o "$2" -D "$tmp/headers" -w '%{http_code}' -H 'accept: text/event-stream' "$base$1" 2>"$tmp/err" || true
  fi
}

open_stream() { # open_stream PATH OUT RC [HEADER]: a stream that has no end of its own, in the background; RC gets curl's exit status when it ends
  : >"$2"
  rm -f "$3"
  if [ $# -ge 4 ]; then
    ( rc=0; curl -sS -N --max-time "$timeout" -o "$2" -H 'accept: text/event-stream' -H "$4" "$base$1" 2>"$2.err" || rc=$?; echo "$rc" >"$3" ) &
  else
    ( rc=0; curl -sS -N --max-time "$timeout" -o "$2" -H 'accept: text/event-stream' "$base$1" 2>"$2.err" || rc=$?; echo "$rc" >"$3" ) &
  fi
  background="$background $!"
}

wait_data() { # wait_data FILE: within 20 s the stream has delivered a frame
  _n=0
  while ! grep -q '^data:' "$1" 2>/dev/null; do
    _n=$((_n + 1))
    [ "$_n" -lt 40 ] || return 1
    sleep 0.5
  done
}

wait_end() { # wait_end RC: within 60 s the stream has ended (curl's status is in the file)
  _n=0
  while [ ! -f "$1" ]; do
    _n=$((_n + 1))
    [ "$_n" -lt 120 ] || return 1
    sleep 0.5
  done
}

run_input() { # run_input THREAD TEXT: the RunAgentInput of a first message
  jq -nc --arg thread "$1" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}'
}

post_run() { # post_run HEADER AGENT INPUT OUT: one run, to its end; prints the HTTP status
  curl -sS -N --max-time "$timeout" -o "$4" -w '%{http_code}' -X POST "$base/agui/agents/$2" -H "$1" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary "@$3" 2>"$tmp/err" || true
}

# --- the cap: restart the orchestrator at `public`, and back ------------------------------------------------------------------------
wait_cap() { # wait_cap CAP: within 120 s the edge is ready and the owner's GET /api/me says the cap
  _n=0
  while :; do
    if [ "$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' "$base/readyz" 2>/dev/null || true)" = 200 ] &&
      [ "$(api GET /api/me 2>/dev/null | jq -r '.sharing // empty' 2>/dev/null || true)" = "$1" ]; then
      return 0
    fi
    _n=$((_n + 1))
    [ "$_n" -lt 120 ] || return 1
    sleep 1
  done
}

set_cap() { # set_cap internal|public: the orchestrator restarted on the stack's own file, or on the copy that says public; waits until it serves
  _up=0
  case $1 in
    public)
      restart_needed=1
      compose_public up -d --no-build --no-deps --wait --wait-timeout 180 orchestrator >"$tmp/up.out" 2>&1 || _up=1 ;;
    *)
      compose up -d --no-build --no-deps --wait --wait-timeout 180 orchestrator >"$tmp/up.out" 2>&1 || _up=1
      [ "$_up" = 1 ] || restart_needed=0 ;;
  esac
  if [ "$_up" = 1 ]; then
    bad "the orchestrator could not be restarted at the cap $1: $(tail -n 5 "$tmp/up.out" | tr '\n' ' ')"
    finish
  fi
  if wait_cap "$1"; then ok "the orchestrator serves again, at the cap $1 (GET /api/me says so)"; else bad "the orchestrator does not serve at the cap $1 within 120 s"; finish; fi
}

# --- what a reader may be given, as checks ---------------------------------------------------------------------------------------
check_view() { # check_view LABEL FILE VISIBILITY ISOWNER: the reader's view of the thread (the contract's SharedThread)
  expect "$1: the view says what the link is served at" "$(jq -r '.visibility // empty' "$2")" "$3"
  expect "$1: ... isOwner is $4" "$(jq -r '.isOwner' "$2")" "$4"
  expect "$1: ... it names the thread" "$(jq -r '.id // empty' "$2")" "$thread"
  _extra=$(jq -r '[keys[]] - ["createdAt","description","id","isOwner","job","lastSeq","state","target","title","updatedAt","visibility"] | join(" ")' "$2" 2>/dev/null || echo '?')
  expect "$1: ... only the keys of a reader (no owner, forkedFrom, tools, share, pinned, archived, nestedUnder)" "${_extra:-none}" none
  if grep -Fq "$email" "$2"; then bad "$1: the owner's e-mail is in the view"; else ok "$1: the owner's e-mail is nowhere in the view"; fi
}

# check_head LABEL SSEFILE VIEWFILE: the replay of a thread whose log ends in its share, which has no frame and so no resume point, ends with
# an `id:` and nothing else, the `lastSeq` the reader's view gave: how a reader tells that its stream holds the whole log (ADR 0040, 2026-10-14)
check_head() {
  _last=$(tr -d '\r' <"$2" | awk 'BEGIN { RS = ""; FS = "\n" } { last = $0 } END { print last }')
  expect "$1: ... the stream says where the log ends: its last block is an id: with no data, the view's lastSeq" "$_last" "id: $(jq -r '.lastSeq' "$3")"
}

# check_frames LABEL FILE AUDIENCE: the replay of the thread for the owner, an internal reader or a public one
# shellcheck disable=SC2016 # jq's own variables, not the shell's
steps_def='def steps: .[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step"); '
check_frames() {
  frames "$2" >"$tmp/frames.json"
  _n=$(jq -r 'length' "$tmp/frames.json")
  if [ "$_n" -gt 0 ]; then ok "$1: the stream replays the thread ($_n events)"; else bad "$1: the stream has no event ($(head -c 200 "$2"))"; return 0; fi
  _said=$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_CONTENT") | .delta] | join(" ")' "$tmp/frames.json")
  case $_said in
    *football*) ok "$1: ... the person's question is in it" ;;
    *) bad "$1: ... the person's question is not in it" ;;
  esac
  _actor=$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "user")] | first | .metadata["vymalo.actor"].name // empty' "$tmp/frames.json")
  if [ "$3" = owner ]; then
    expect "$1: ... the person's message is sent by them (their e-mail)" "$_actor" "$email"
    if grep -Fq "$email" "$2"; then ok "$1: ... the owner's e-mail is in their own stream (the control for the readers' streams)"; else bad "$1: ... the owner's e-mail is not even in their own stream: the checks below would prove nothing"; fi
  else
    expect "$1: ... the person's message is sent by \"the owner\"" "$_actor" "the owner"
    if grep -Fq "$email" "$2"; then bad "$1: ... the owner's e-mail is in the stream"; else ok "$1: ... the owner's e-mail is nowhere in the stream"; fi
  fi
  _steps=$(jq -r "$steps_def"'[steps | select(.content.kind == "tool" and .content.label == "Web search")] | length' "$tmp/frames.json")
  if [ "$_steps" -ge 1 ]; then ok "$1: ... the step is there, with its label (Web search)"; else bad "$1: ... no step labelled Web search"; fi
  if [ "$3" = public ]; then
    expect "$1: ... no step has an input, an output or a detail" \
      "$(jq -r "$steps_def"'[steps | .content | (.input, .output, .detail) | select(. != null)] | length' "$tmp/frames.json")" 0
  else
    _query=$(jq -r "$steps_def"'[steps | .content.input.query // empty] | first // empty' "$tmp/frames.json")
    case $_query in
      *football*) ok "$1: ... the step has its input (the query: $_query)" ;;
      *) bad "$1: ... the step has no input with the query (got '$_query')" ;;
    esac
    _out=$(jq -r "$steps_def"'[steps | .content.output.text // empty] | last // empty' "$tmp/frames.json")
    case $_out in
      *example.org/mock-search/*) ok "$1: ... and its output (the mock's links)" ;;
      *) bad "$1: ... the step has no output with the mock's links (got '$_out')" ;;
    esac
  fi
}

# edge_negatives LABEL TOKEN: what an anonymous request must NOT reach, with a link that exists. All of it is the edge's answer.
edge_negatives() {
  _t=$2
  expect "$1: GET / is sent to sign in" "$(anon GET /)" 302
  expect "$1: ... to /oauth2/start, back to /" "$(tr -d '\r' <"$tmp/headers" | awk 'tolower($1) == "location:" { print $2 }')" "/oauth2/start?rd=/"
  expect "$1: GET /threads/<id> is sent to sign in" "$(anon GET "/threads/$thread")" 302
  expect "$1: ... and back to it" "$(tr -d '\r' <"$tmp/headers" | awk 'tolower($1) == "location:" { print $2 }')" "/oauth2/start?rd=/threads/$thread"
  expect "$1: GET /api/threads is 401" "$(anon GET /api/threads)" 401
  expect "$1: GET /api/threads/<id> is 401" "$(anon GET "/api/threads/$thread")" 401
  expect "$1: GET /api/me is 401" "$(anon GET /api/me)" 401
  expect "$1: GET /agui/threads/<id>/connect is 401" "$(anon GET "/agui/threads/$thread/connect" -H 'accept: text/event-stream')" 401
  expect "$1: POST /agui/agents/$agent is 401" "$(anon POST "/agui/agents/$agent" -H 'content-type: application/json' -d '{}')" 401
  expect "$1: the signed-in link, GET /api/shared/<token>, is 401" "$(anon GET "/api/shared/$_t")" 401
  expect "$1: the signed-in stream, GET /agui/shared/<token>/connect, is 401" "$(anon GET "/agui/shared/$_t/connect" -H 'accept: text/event-stream')" 401
  expect "$1: a POST to the public link is 401 (only GET and HEAD are let through)" "$(anon POST "/api/public/shared/$_t")" 401
  expect "$1: a DELETE of the public link is 401" "$(anon DELETE "/api/public/shared/$_t")" 401
  expect "$1: a POST to the public stream is 401" "$(anon POST "/agui/public/shared/$_t/connect")" 401
  expect "$1: a public route that is not shared/ (/api/public/other) is 401" "$(anon GET /api/public/other)" 401
    expect "$1: a path that climbs out of shared/ (../..) is 401" "$(anon GET "/api/public/shared/$_t/../../threads" --path-as-is)" 401
  expect "$1: ... also with its dots encoded" "$(anon GET "/api/public/shared/$_t/%2e%2e/%2e%2e/threads" --path-as-is)" 401
  _code=$(anon GET "/API/PUBLIC/SHARED/$_t")
  case $_code in
    401 | 404) ok "$1: another casing of the public path does not open the link (the edge's path matcher ignores case, the orchestrator's routes do not: $_code)" ;;
    *) bad "$1: GET /API/PUBLIC/SHARED/<token> answered $_code, want 401 or 404" ;;
  esac
  expect "$1: ... up to /api/threads itself" "$(anon GET "/api/public/shared/$_t/../../../threads" --path-as-is)" 401
  expect "$1: ... and for the stream route" "$(anon GET "/agui/public/shared/$_t/../../threads/$thread/connect" --path-as-is)" 401
}

# --- the stack ------------------------------------------------------------------------------------------------------------------
if [ -z "$(compose ps -q orchestrator 2>/dev/null || true)" ]; then
  echo "SKIP  the orchestrator is not a service of this compose project (start it: docker compose --profile app up -d --build --wait)." >&2
  exit 77
fi
agents=$(api GET /api/agents | jq -r '[.[].id] | join(" ")')
case " $agents " in
  *" $agent "*) ;;
  *) echo "the agent '$agent' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac
cap=$(api GET /api/me | jq -r '.sharing // "disabled"')
if [ "$cap" != internal ]; then
  echo "the stack's cap is '$cap' (GET /api/me), the script starts at 'internal' (sharing.mode of dev/orchestrator.yaml). If an earlier run left it at 'public': docker compose --profile app up -d --no-build --no-deps orchestrator" >&2
  exit 2
fi
# The copy of the stack's file that says `public`: exactly one line differs, or the file is not the one this script knows.
awk '/^sharing:$/ { s = 1 } s && /^  mode: internal$/ { print "  mode: public"; s = 0; next } { print }' "$config" >"$tmp/orchestrator.public.yaml"
if [ "$(diff "$config" "$tmp/orchestrator.public.yaml" | grep -c '^[<>]' || true)" -ne 2 ]; then
  echo "$config has no 'sharing:' section with the line '  mode: internal' (ORCH_CONFIG): the copy with the cap 'public' cannot be made" >&2
  exit 2
fi
chmod a+r "$tmp/orchestrator.public.yaml"
cat >"$tmp/compose.public.yaml" <<EOF
# Generated by dev/share-e2e.sh: the orchestrator's configuration mounted where the stack's own file is, at the cap \`public\`.
services:
  orchestrator:
    volumes:
      - "$tmp/orchestrator.public.yaml:/etc/orchestrator/config.yaml:ro"
EOF

# --- a thread to share ----------------------------------------------------------------------------------------------------------
echo "== a thread with a tool step, as the owner has it"
thread=$(uuid)
run_input "$thread" "Who won the football world cup in 2014?" >"$tmp/run.json"
expect "the run is accepted" "$(post_run "$id_header" "$agent" "$tmp/run.json" "$tmp/run.sse")" 200
if wait_state "$thread" "done"; then ok "the thread ends done"; else bad "the thread did not end done (it is '$(state_of "$thread")')"; finish; fi
settle "$thread"
expect "the owner's view of it names the owner (the control: a reader must not)" "$(api GET "/api/threads/$thread" | jq -r '.owner')" "$email"
expect "it is private: no share in the owner's view" "$(api GET "/api/threads/$thread" | jq -r 'has("share")')" false
expect "the owner's own stream of it is 200" "$(sse_get "/agui/threads/$thread/connect?mode=run" "$tmp/owner.sse" "$id_header")" 200
check_frames "the owner" "$tmp/owner.sse" owner
bogus=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA

# ==============================================================================================================================
echo "== at the cap internal: the owner shares, another signed-in user reads, and cannot write"
expect "the stack's cap is internal for the owner (GET /api/me)" "$(api GET /api/me | jq -r '.sharing')" internal
expect "sharing as internal is 200" "$(call "$id_header" PUT "/api/threads/$thread/share" '{"visibility":"internal"}')" 200
expect "... it says internal and internal" "$(jq -r '[.visibility, .effective] | join(" ")' "$tmp/body")" "internal internal"
token_a=$(jq -r '.url // empty | sub("^.*/s/"; "")' "$tmp/body")
expect "... the link is /s/<token>, a token of 43 characters (32 bytes)" "${#token_a}" 43
case $(jq -r '.url // empty' "$tmp/body") in
  */s/"$token_a") ok "... the link ends with the token" ;;
  *) bad "... the link is not <origin>/s/<token>: $(jq -r '.url // empty' "$tmp/body")" ;;
esac
expect "... the owner's thread says it is shared, with the link" "$(api GET "/api/threads/$thread" | jq -r '[.share.visibility, .share.effective, (.share.url | endswith($t))] | join(" ")' --arg t "/s/$token_a")" "internal internal true"
api GET "/api/threads/$thread/export" >"$tmp/export.json" || echo '{"events":[]}' >"$tmp/export.json"
expect "the log has the share, as one thread_shared event" "$(jq -r '[.events[] | select(.kind == "thread_shared")] | length' "$tmp/export.json")" 1
if grep -Fq "$token_a" "$tmp/export.json"; then bad "the export holds the token: the log must never hold a capability"; else ok "the export holds no token (the log has the digest of the nonce, never the link)"; fi

expect "another user opens the link: 200" "$(call "$other_header" GET "/api/shared/$token_a")" 200
expect "... no-store" "$(header cache-control)" "no-store"
expect "... noindex" "$(header x-robots-tag)" "noindex, nofollow"
cp "$tmp/body" "$tmp/internal-view.json"
check_view "the second user's view" "$tmp/internal-view.json" internal false
expect "the owner opens their own link: 200" "$(call "$id_header" GET "/api/shared/$token_a")" 200
cp "$tmp/body" "$tmp/owner-view.json"
expect "... as the owner (isOwner)" "$(jq -r '.isOwner' "$tmp/owner-view.json")" true
expect "the second user's stream of the link is 200" "$(sse_get "/agui/shared/$token_a/connect?mode=run" "$tmp/internal.sse" "$other_header")" 200
case $(header content-type) in
  text/event-stream*) ok "... an event stream" ;;
  *) bad "... the content type is '$(header content-type)', not an event stream" ;;
esac
case $(header cache-control) in
  *no-store*) ok "... no-store" ;;
  *) bad "... the stream is cacheable ('$(header cache-control)')" ;;
esac
check_frames "the second user's stream" "$tmp/internal.sse" internal
check_head "the second user's stream" "$tmp/internal.sse" "$tmp/internal-view.json"

echo "== at the cap internal: nobody is anonymous, and the public routes answer the one 404"
expect "an anonymous GET of the signed-in link is 401, at the edge" "$(anon GET "/api/shared/$token_a")" 401
expect "an anonymous GET of the public link of an internal thread is 404 (it reaches the orchestrator)" "$(anon GET "/api/public/shared/$token_a")" 404
cp "$tmp/body" "$tmp/404-internal.json"
expect "... application/problem+json, no-store, noindex" "$(header content-type)|$(header cache-control)|$(header x-robots-tag)" "application/problem+json|no-store|noindex, nofollow"
expect "a guess is a 404 too" "$(anon GET "/api/public/shared/$bogus")" 404
expect "... with the same body (one 404 for every cause)" "$(jq -cS . "$tmp/body")" "$(jq -cS . "$tmp/404-internal.json")"
expect "a guess with a bearer that is no token and an identity header is the same 404 (the edge asks nobody on the public routes)" \
  "$(anon GET "/api/public/shared/$bogus" -H 'Authorization: Bearer not-a-token' -H 'X-Auth-Request-Email: admin@example.com')" 404
expect "the public stream of an internal thread is 404" "$(anon GET "/agui/public/shared/$token_a/connect" -H 'accept: text/event-stream')" 404
expect "the signed-in link of a guess is 404 for a signed-in user" "$(call "$other_header" GET "/api/shared/$bogus")" 404
expect "... with the same body" "$(jq -cS . "$tmp/body")" "$(jq -cS . "$tmp/404-internal.json")"
expect "the owner shares as public: 409 over the cap" "$(call "$id_header" PUT "/api/threads/$thread/share" '{"visibility":"public"}')" 409
expect "... over_cap" "$(jq -r '.code' "$tmp/body")" over_cap
expect "... and the thread is still internal" "$(api GET "/api/threads/$thread" | jq -r '.share.visibility')" internal

echo "== at the cap internal: the edge lets nothing else through"
edge_negatives "anonymous, internal link" "$token_a"
for p in /s/"$token_a" /favicon.ico /icon.svg /apple-icon.png /manifest.webmanifest /brand/none.svg /_next/static/share-e2e-none.js; do
  _code=$(anon GET "$p")
  case $_code in
    200 | 404) ok "anonymous GET $(printf '%s' "$p" | sed "s|$token_a|<token>|") reaches the web with no sign-in ($_code, not the edge's redirect)" ;;
    *) bad "anonymous GET $(printf '%s' "$p" | sed "s|$token_a|<token>|") is $_code, want the web's own answer (200 or 404)" ;;
  esac
done
expect "the page of the link is served with no sign-in (200)" "$(anon GET "/s/$token_a")" 200
case $(header content-type) in
  text/html*) ok "... as HTML" ;;
  *) bad "... the content type is '$(header content-type)'" ;;
esac
expect "... and keeps the link out of other sites' referrers (Referrer-Policy)" "$(header referrer-policy)" same-origin

echo "== at the cap internal: any write by the second user is refused, and changes nothing"
before=$(api GET "/api/threads/$thread" | jq -cS '{state, title, lastSeq, share}')
expect "share as public: 404" "$(call "$other_header" PUT "/api/threads/$thread/share" '{"visibility":"public"}')" 404
expect "share as internal: 404" "$(call "$other_header" PUT "/api/threads/$thread/share" '{"visibility":"internal"}')" 404
expect "a new link: 404" "$(call "$other_header" POST "/api/threads/$thread/share/rotate")" 404
expect "take the link down: 404" "$(call "$other_header" DELETE "/api/threads/$thread/share")" 404
expect "rename it: 404" "$(call "$other_header" PATCH "/api/threads/$thread" '{"title":"taken over"}')" 404
expect "cancel it: 404" "$(call "$other_header" POST "/api/threads/$thread/cancel")" 404
expect "fork it: 404" "$(call "$other_header" POST "/api/threads/$thread/fork" '{"after":1}')" 404
expect "delete it: 404" "$(call "$other_header" DELETE "/api/threads/$thread")" 404
run_input "$thread" "hello from the reader" >"$tmp/reader-run.json"
expect "send it a message over AG-UI: 404" "$(post_run "$other_header" "$agent" "$tmp/reader-run.json" "$tmp/reader-run.sse")" 404
expect "read it by its id: 404" "$(call "$other_header" GET "/api/threads/$thread")" 404
expect "export it: 404" "$(call "$other_header" GET "/api/threads/$thread/export")" 404
expect "stream it by its id: 404" "$(sse_get "/agui/threads/$thread/connect" "$tmp/reader-id.sse" "$other_header")" 404
expect "it is not in their list" "$(call "$other_header" GET "/api/threads?limit=100" >/dev/null; jq -r --arg id "$thread" '[.[] | select(.id == $id)] | length' "$tmp/body")" 0
for m in POST PUT PATCH DELETE; do
  expect "$m on the link itself: 405" "$(call "$other_header" "$m" "/api/shared/$token_a" '{}')" 405
done
after=$(api GET "/api/threads/$thread" | jq -cS '{state, title, lastSeq, share}')
expect "the thread is as it was (state, title, last event, share)" "$after" "$before"
expect "and the link still opens for them" "$(call "$other_header" GET "/api/shared/$token_a")" 200

echo "== a new link kills the old one"
expect "POST /share/rotate is 200" "$(call "$id_header" POST "/api/threads/$thread/share/rotate")" 200
token_b=$(jq -r '.url // empty | sub("^.*/s/"; "")' "$tmp/body")
if [ -n "$token_b" ] && [ "$token_b" != "$token_a" ]; then ok "... the link is another one"; else bad "... the link did not change ($token_b)"; fi
expect "the old link is a 404 for the second user" "$(call "$other_header" GET "/api/shared/$token_a")" 404
expect "the new link opens for them" "$(call "$other_header" GET "/api/shared/$token_b")" 200
expect "the thread is still internal" "$(api GET "/api/threads/$thread" | jq -r '.share.visibility')" internal

echo "== at the cap internal: revoking ends the stream open on the link"
open_stream "/agui/shared/$token_b/connect" "$tmp/open-internal.sse" "$tmp/open-internal.rc" "$other_header"
if wait_data "$tmp/open-internal.sse"; then ok "the second user's stream, open with no end of its own, has delivered its frames through the edge"; else bad "the open stream delivered nothing in 20 s ($(head -c 200 "$tmp/open-internal.sse.err"))"; fi
expect "DELETE /share is 204" "$(call "$id_header" DELETE "/api/threads/$thread/share")" 204
if wait_end "$tmp/open-internal.rc"; then
  _rc=$(cat "$tmp/open-internal.rc")
  if [ "$_rc" = 28 ]; then bad "the open stream was still open when curl gave up (exit 28)"; else ok "the open stream ENDED by itself (curl exit $_rc)"; fi
else
  bad "the open stream did not end within 60 s of the revocation"
fi
expect "the link is a 404 for the second user" "$(call "$other_header" GET "/api/shared/$token_b")" 404
expect "its stream is a 404" "$(sse_get "/agui/shared/$token_b/connect" "$tmp/gone.sse" "$other_header")" 404
expect "... and for the owner reading their own link" "$(call "$id_header" GET "/api/shared/$token_b")" 404
expect "the public route is a 404 too" "$(anon GET "/api/public/shared/$token_b")" 404
expect "the thread is private again: no share in the owner's view" "$(api GET "/api/threads/$thread" | jq -r 'has("share")')" false
api GET "/api/threads/$thread/export" >"$tmp/export.json" || echo '{"events":[]}' >"$tmp/export.json"
expect "the log has the revocation: a thread_unshared event" "$(jq -r '[.events[] | select(.kind == "thread_unshared")] | length' "$tmp/export.json")" 1
expect "a second DELETE is 204 (nothing to take down)" "$(call "$id_header" DELETE "/api/threads/$thread/share")" 204
expect "sharing it again is 200" "$(call "$id_header" PUT "/api/threads/$thread/share" '{"visibility":"internal"}')" 200
token_c=$(jq -r '.url // empty | sub("^.*/s/"; "")' "$tmp/body")
if [ -n "$token_c" ] && [ "$token_c" != "$token_a" ] && [ "$token_c" != "$token_b" ]; then ok "... with a new link: a revoked one never comes back"; else bad "... the link is one of the earlier ones"; fi
expect "the revoked link is still a 404" "$(call "$other_header" GET "/api/shared/$token_b")" 404
expect "and the new one opens" "$(call "$other_header" GET "/api/shared/$token_c")" 200

# ==============================================================================================================================
echo "== the cap public: the orchestrator is restarted on a copy of its own file with sharing.mode public"
set_cap public
expect "GET /api/me says public" "$(api GET /api/me | jq -r '.sharing')" public
expect "the link made at the cap internal is the same link (the nonce is in the row, the secret is the stack's)" "$(call "$other_header" GET "/api/shared/$token_c")" 200
expect "widening it to public is 200" "$(call "$id_header" PUT "/api/threads/$thread/share" '{"visibility":"public"}')" 200
expect "... public and public" "$(jq -r '[.visibility, .effective] | join(" ")' "$tmp/body")" "public public"
expect "... and it is THE SAME LINK (a widening keeps the nonce)" "$(jq -r '.url | sub("^.*/s/"; "")' "$tmp/body")" "$token_c"

echo "== the cap public: an anonymous curl reads the thread"
expect "GET /api/public/shared/<token> with no credential is 200" "$(anon GET "/api/public/shared/$token_c")" 200
expect "... json" "$(header content-type | cut -d';' -f1)" application/json
expect "... no-store" "$(header cache-control)" "no-store"
expect "... noindex" "$(header x-robots-tag)" "noindex, nofollow"
cp "$tmp/body" "$tmp/public-view.json"
check_view "the anonymous view" "$tmp/public-view.json" public false
expect "the same request with the owner's bearer token changes nothing: isOwner false" \
  "$(anon GET "/api/public/shared/$token_c" -H "$id_header" -H "X-Auth-Request-Email: $email" >/dev/null; jq -r '.isOwner' "$tmp/body")" false
expect "... and the answer is as anybody's, down to the keys" "$(jq -r 'keys | join(",")' "$tmp/body")" "$(jq -r 'keys | join(",")' "$tmp/public-view.json")"
expect "a bearer that is no token is ignored: 200, not the 401 oauth2-proxy would give" "$(anon GET "/api/public/shared/$token_c" -H 'Authorization: Bearer not-a-token')" 200
expect "the public replay, GET /agui/public/shared/<token>/connect with no credential, is 200" "$(sse_get "/agui/public/shared/$token_c/connect?mode=run" "$tmp/public.sse")" 200
case $(header content-type) in
  text/event-stream*) ok "... an event stream" ;;
  *) bad "... the content type is '$(header content-type)'" ;;
esac
case $(header cache-control) in
  *no-store*) ok "... no-store" ;;
  *) bad "... the stream is cacheable ('$(header cache-control)')" ;;
esac
expect "... noindex" "$(header x-robots-tag)" "noindex, nofollow"
check_frames "the anonymous stream" "$tmp/public.sse" public
check_head "the anonymous stream" "$tmp/public.sse" "$tmp/public-view.json"
expect "a signed-in user's stream of the same public link is 200" "$(sse_get "/agui/shared/$token_c/connect?mode=run" "$tmp/public-signed-in.sse" "$other_header")" 200
check_frames "the signed-in reader of the public link" "$tmp/public-signed-in.sse" internal
check_head "the signed-in reader of the public link" "$tmp/public-signed-in.sse" "$tmp/public-view.json"
expect "files are off for a public reader (sharing.public.files): the route is the one 404" \
  "$(anon GET "/api/public/shared/$token_c/artifacts/0000000000000000000000000000000000000000000000000000000000000000")" 404
expect "the page of the link is served with no sign-in" "$(anon GET "/s/$token_c")" 200
edge_negatives "anonymous, public link" "$token_c"

echo "== the cap public: a fork of the shared thread is private"
settle "$thread"
last_seq=$(api GET "/api/threads/$thread/export" | jq -r '[.events[].seq] | max // 0')
fork=$(uuid)
expect "forking the shared thread is 201" "$(call "$id_header" POST "/api/threads/$thread/fork" "$(jq -n --arg id "$fork" --argjson after "$last_seq" '{after: $after, id: $id}')")" 201
expect "the fork says where it came from" "$(api GET "/api/threads/$fork" | jq -r '.forkedFrom.threadId')" "$thread"
expect "the fork's log carries the copy of the share event, as history (the control)" \
  "$(api GET "/api/threads/$fork/export" | jq -r '[.events[] | select(.kind == "thread_shared")] | length >= 1')" true
expect "the fork is PRIVATE: no share in its owner's view" "$(api GET "/api/threads/$fork" | jq -r 'has("share")')" false
expect "... nor in the owner's list" "$(api GET '/api/threads?limit=100' | jq -r --arg id "$fork" '[.[] | select(.id == $id and has("share"))] | length')" 0
expect "the second user cannot read it (404)" "$(call "$other_header" GET "/api/threads/$fork")" 404
expect "an anonymous request cannot (401)" "$(anon GET "/api/threads/$fork")" 401
expect "the parent's public link still shows the parent, not the fork" "$(anon GET "/api/public/shared/$token_c")" 200
expect "... the parent's id" "$(jq -r '.id' "$tmp/body")" "$thread"

echo "== the cap public: revoking ends the stream an anonymous reader has open"
open_stream "/agui/public/shared/$token_c/connect" "$tmp/open-public.sse" "$tmp/open-public.rc"
if wait_data "$tmp/open-public.sse"; then ok "the anonymous stream, open with no end of its own, has delivered its frames through the edge"; else bad "the open public stream delivered nothing in 20 s ($(head -c 200 "$tmp/open-public.sse.err"))"; fi
expect "DELETE /share is 204" "$(call "$id_header" DELETE "/api/threads/$thread/share")" 204
if wait_end "$tmp/open-public.rc"; then
  _rc=$(cat "$tmp/open-public.rc")
  if [ "$_rc" = 28 ]; then bad "the open public stream was still open when curl gave up (exit 28)"; else ok "the open public stream ENDED by itself (curl exit $_rc)"; fi
else
  bad "the open public stream did not end within 60 s of the revocation"
fi
expect "the public link is a 404 for anybody" "$(anon GET "/api/public/shared/$token_c")" 404
expect "... the public stream too" "$(anon GET "/agui/public/shared/$token_c/connect" -H 'accept: text/event-stream')" 404
expect "... with the body of every other 404" "$(anon GET "/api/public/shared/$token_c" >/dev/null; jq -cS . "$tmp/body")" "$(jq -cS . "$tmp/404-internal.json")"
expect "the signed-in link is a 404 for the second user" "$(call "$other_header" GET "/api/shared/$token_c")" 404
expect "the thread is private again" "$(api GET "/api/threads/$thread" | jq -r 'has("share")')" false
expect "the fork never had a link, and the parent's revocation did not give it one" "$(api GET "/api/threads/$fork" | jq -r 'has("share")')" false

# ==============================================================================================================================
echo "== the cap back at internal: a link that was made public is a 404 on the public routes"
expect "sharing the thread as public again is 200 (the cap is still public)" "$(call "$id_header" PUT "/api/threads/$thread/share" '{"visibility":"public"}')" 200
token_d=$(jq -r '.url // empty | sub("^.*/s/"; "")' "$tmp/body")
expect "... and an anonymous curl reads it" "$(anon GET "/api/public/shared/$token_d")" 200
set_cap internal
expect "GET /api/me says internal again" "$(api GET /api/me | jq -r '.sharing')" internal
expect "the public link is a 404 for an anonymous curl, at the orchestrator (the cap narrowed it)" "$(anon GET "/api/public/shared/$token_d")" 404
expect "... with the one 404 body" "$(jq -cS . "$tmp/body")" "$(jq -cS . "$tmp/404-internal.json")"
expect "... the public stream too" "$(anon GET "/agui/public/shared/$token_d/connect" -H 'accept: text/event-stream')" 404
expect "a signed-in user still reads it, served as internal" "$(call "$other_header" GET "/api/shared/$token_d")" 200
expect "... visibility internal" "$(jq -r '.visibility' "$tmp/body")" internal
expect "the owner's view says public stored and internal served: nothing was rewritten" "$(api GET "/api/threads/$thread" | jq -r '[.share.visibility, .share.effective] | join(" ")')" "public internal"
expect "the page is still served with no sign-in (the edge does not know the cap)" "$(anon GET "/s/$token_d")" 200
edge_negatives "anonymous, public link under the cap internal" "$token_d"
expect "taking the link down is 204" "$(call "$id_header" DELETE "/api/threads/$thread/share")" 204
expect "the thread is private" "$(api GET "/api/threads/$thread" | jq -r 'has("share")')" false

for _pid in $background; do wait "$_pid" 2>/dev/null || true; done
finish
