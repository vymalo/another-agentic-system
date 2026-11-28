#!/usr/bin/env sh
# System-level test of the prompt signature at intake (ADR 0087, the cloud path's note of 2026-11-25): the orchestrator does not
# verify the person's signature. A prompt from a **signing client** — a token whose `azp` (or `client_id`) is one of
# `auth.signingClients` of dev/orchestrator.yaml — must carry the envelope in the new message's `metadata["vymalo.promptSignature"]`:
# a compact JWS with a detached payload, three base64url parts. One that carries none, or one of another shape, is a `403
# prompt_signature_invalid` with `reason: invalid`, before anything is written and before any agent is asked. A client that is not
# listed is never asked, and a deployment that lists none asks nobody.
#
#   dev/prompt-signature-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-coder` (dev/agents.yaml), the WireMock A2A mock of `dev/wiremock/agent`: it answers a message with a pull
# request and `completed`, so the thread of an accepted prompt ends `done`, and its request journal (`GET /__admin/requests`) says
# whether a refused prompt reached it. Nothing checks the bytes of the envelope (the verifier behind the model gateway does): the
# script sends a well-formed compact JWS with a detached payload and asserts only where it goes and where it does not.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the two tokens: the mock issuer's dev-only `azp` extension (dev/auth-header.sh, `OIDC_AZP`) mints one whose `azp` is
#     `dev-signing` — the one client `auth.signingClients` names — and another whose `azp` is `dev-chat`, the client every other
#     scenario authenticates as; they are the same person;
#   * as the signing client, a run whose new message carries no envelope is a 403 `prompt_signature_invalid`, `reason: invalid`,
#     whose detail is the person's sentence; the thread does not exist (404) and `mock-coder` was sent nothing;
#   * the same for a malformed envelope — `a.b` (two parts), `a..b.c` (four) and one over 16 KiB — and the problem's body holds
#     none of it;
#   * a helper-session start (`POST /api/threads/{id}/sessions`, a random thread) from the signing client with no envelope is the
#     same 403, and the same request from `dev-chat` is a 404: the check comes before the thread is looked up;
#   * a well-formed detached JWS (`eyJhbGciOiJFUzI1NiJ9..c2ln`) is accepted (200), the thread runs to `done`, and `mock-coder` was
#     sent the prompt exactly once;
#   * a token whose `azp` is `dev-chat` sends no envelope and is accepted (200), and its thread ends `done` (the negative control);
#   * the envelope, and the part of it that names its header, are in no export and in no AG-UI frame of the thread that carried it,
#     live and replayed: the log keeps no envelope.
# It ends with the count of its ok and FAIL lines, and its exit status is 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   MOCK_AGENT_URL  http://127.0.0.1:${MOCK_AGENT_PORT:-8081}, where the admin API of the WireMock `mock-agent` is
#   TIMEOUT         120    seconds to wait for a thread to stop
#
# It signs in as `prompt-signature@example.com`, a user no other scenario uses, and UNSETS `OIDC_ROLES`, `OIDC_AZP`, `OIDC_AUDIENCE`
# and `AUTH_BEARER` of your shell: any of them would change every token the script asks for, and the two `azp` values are what the
# scenario is about. It EMPTIES the request journal of `mock-agent` first, so run it on a stack you are not in the middle of another
# scenario on. Needs curl and jq (and python3, or /proc, uuidgen for a v7 UUID). Run against the `app` profile built from this branch on
# 2026-11-28 (39 ok, no FAIL), before the four checks of the helper-session start were added, which have not been run there; CI
# runs it too, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
here=$(cd "$(dirname "$0")" && pwd)
email=prompt-signature@example.com
agent="mock-coder"
mock=${MOCK_AGENT_URL:-http://127.0.0.1:${MOCK_AGENT_PORT:-8081}}
mock=${mock%/}
timeout=${TIMEOUT:-120}

pass=0
fail=0
ok() { pass=$((pass + 1)); echo "ok   $1"; }
bad() { fail=$((fail + 1)); echo "FAIL $1"; }
finish() {
  echo "$pass ok, $fail FAIL"
  if [ "$fail" -eq 0 ]; then echo "prompt signature e2e passed"; else echo "prompt signature e2e FAILED"; exit 1; fi
}
expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for tool in curl jq base64; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

uuid() { python3 -c 'import os,time;m=time.time_ns()//1000000;print("%08x-%04x-7%03x-%04x-%012x"%(m>>16,m&65535,int.from_bytes(os.urandom(2),"big")&4095,32768|int.from_bytes(os.urandom(2),"big")&16383,int.from_bytes(os.urandom(6),"big")))' 2>/dev/null || cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# claims_of HEADER: the claims of the bearer token in HEADER, as JSON (a JWT's payload is base64url, unpadded).
claims_of() {
  _token=${1#Authorization: Bearer }
  _payload=$(printf '%s' "$_token" | cut -d. -f2 | tr '_-' '/+')
  case $((${#_payload} % 4)) in
    2) _payload="${_payload}==" ;;
    3) _payload="${_payload}=" ;;
  esac
  printf '%s' "$_payload" | base64 -d 2>/dev/null || true
}

reset_journal() { # reset_journal: empties the mock agent's request journal
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$mock/__admin/requests" || true)
  if [ "$_code" = 200 ]; then ok "the journal of mock-coder is empty"; else bad "the journal of mock-coder could not be emptied: HTTP $_code at $mock"; fi
}

# sent TEXT: how many A2A `SendStreamingMessage` requests the mock was sent whose text holds TEXT (WireMock's journal).
sent() {
  _journal=$(curl -fsS --max-time 30 "$mock/__admin/requests" 2>/dev/null) || { echo unreadable; return 0; }
  printf '%s' "$_journal" | jq -r --arg t "$1" '
    [.requests[] | .request.body | fromjson? // {}
     | select(.method == "SendStreamingMessage"
              and ([.params.message.parts[]?.text // ""] | join(" ") | contains($t)))] | length' 2>/dev/null || echo unreadable
}

# run_agui HEADER THREAD TEXT ENVELOPE: one run (one message) to its end, or its refusal. Prints the HTTP status; the stream, or
# the problem, is $tmp/run.out. An empty ENVELOPE is a message with no metadata member at all.
run_agui() {
  _message=$(jq -n --arg id "$(uuid)" --arg text "$3" --arg envelope "$4" '
    if $envelope == "" then {id: $id, role: "user", content: $text}
    else {id: $id, role: "user", content: $text, metadata: {"vymalo.promptSignature": $envelope}} end')
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --argjson message "$_message" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [$message], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.out" -w '%{http_code}' -X POST "$base/agui/agents/$agent" -H "$1" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

# problem FIELD: that member of the problem in $tmp/run.out (empty when the body is not one).
problem() { jq -r "$1 // empty" "$tmp/run.out" 2>/dev/null || true; }

# status_of HEADER PATH: the HTTP status of a GET of the resource API.
status_of() { curl -sS --max-time 60 -o /dev/null -w '%{http_code}' -H "$1" "$base$2"; }

# get HEADER PATH: the body of a GET of the resource API, non-zero when the status is not 2xx.
get() { curl --fail-with-body -sS --max-time 60 -H "$1" "$base$2"; }

# wait_state THREAD HEADER: waits (up to TIMEOUT seconds) until the thread stops, and prints the state it ended in.
wait_state() {
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(get "$2" "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' 2>/dev/null || true)
    case $_state in done | blocked | failed | cancelled) echo "$_state"; return 0 ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then echo "${_state:-unknown}"; return 0; fi
    sleep 2
  done
}

# frames THREAD HEADER FILE: the thread's AG-UI events, replayed from the start, as one JSON array in FILE.
frames() {
  curl -sS --max-time 60 -H "$2" -H 'accept: text/event-stream' "$base/agui/threads/$1/connect?mode=run" 2>/dev/null |
    sed -n 's/^data: *//p' | jq -s '.' >"$3" 2>/dev/null || echo '[]' >"$3"
}

# --- who, and the two tokens -------------------------------------------------------------------------------------------
# A developer's shell must not decide the tokens: AUTH_BEARER replaces the issuer's token with one of theirs, OIDC_ROLES gives every
# token the same roles, OIDC_AUDIENCE another audience, and OIDC_AZP the same client. The two clients are what this scenario is about.
unset OIDC_ROLES OIDC_AZP OIDC_AUDIENCE AUTH_BEARER
no_token() {
  echo "the mock issuer gave no token for $1 (is mock-oidc up? OIDC_URL=${OIDC_URL:-http://127.0.0.1:${MOCK_OIDC_PORT:-8099}})" >&2
  exit 2
}
signing=$(OIDC_AZP=dev-signing sh "$here/auth-header.sh" "$email") || no_token "the signing client"
control=$(OIDC_AZP=dev-chat sh "$here/auth-header.sh" "$email") || no_token "the negative control"
expect "the token the script signs with says azp dev-signing, the one client auth.signingClients names" \
  "$(claims_of "$signing" | jq -r '.azp // "-"')" "dev-signing"
expect "the control's token says azp dev-chat, which the list does not name" \
  "$(claims_of "$control" | jq -r '.azp // "-"')" "dev-chat"
expect "both tokens are the same person: only the azp changes" \
  "$(claims_of "$signing" | jq -r '.sub // "-"')" "$(claims_of "$control" | jq -r '.sub // "-"')"

# --- a signing client's prompt with no envelope ------------------------------------------------------------------------
echo "== a signing client (azp dev-signing) with no envelope"
reset_journal
refused="refused-$(uuid)"
thread=$(uuid)
code=$(run_agui "$signing" "$thread" "add a health endpoint ($refused)" "")
expect "a prompt with no envelope is a 403" "$code" "403"
expect "  its code is prompt_signature_invalid" "$(problem .code)" "prompt_signature_invalid"
expect "  its reason is invalid" "$(problem .reason)" "invalid"
expect "  its detail is the person's sentence" "$(problem .detail | grep -c 'couldn.t be verified' || true)" "1"
expect "  no thread was written" "$(status_of "$signing" "/api/threads/$thread")" "404"

# --- a signing client's prompt with a malformed envelope ---------------------------------------------------------------
echo "== a signing client (azp dev-signing) with a malformed envelope"
long="a..$(printf '%17000s' '' | tr ' ' 'c')"
for entry in "two parts:a.b" "four parts:a..b.c" "over 16 KiB:$long"; do
  why=${entry%%:*}
  envelope=${entry#*:}
  thread=$(uuid)
  code=$(run_agui "$signing" "$thread" "add a health endpoint ($refused)" "$envelope")
  expect "an envelope of $why is a 403" "$code" "403"
  expect "  its code is prompt_signature_invalid ($why)" "$(problem .code)" "prompt_signature_invalid"
  expect "  its reason is invalid ($why)" "$(problem .reason)" "invalid"
  expect "  the problem holds none of the envelope ($why)" "$(grep -c -F "$envelope" "$tmp/run.out" || true)" "0"
  expect "  no thread was written ($why)" "$(status_of "$signing" "/api/threads/$thread")" "404"
done
expect "mock-coder was sent none of the refused prompts" "$(sent "$refused")" "0"

# --- a signing client's helper-session start with no envelope ---------------------------------------------------------
# `startChildSession` is given `auth.signingClients` separately from the run route (boot.rs), and an empty list checks nobody, so
# it is asked here too. The thread does not exist: the 403 comes before the lookup, so no agent with helpers is needed. The
# control's 404 for the same request says the thread is what a request that is let through finds.
echo "== a signing client (azp dev-signing) starting a helper session with no envelope"
thread=$(uuid)
start=$(jq -n --arg id "$(uuid)" --arg text "add a health endpoint ($refused)" \
  '{id: $id, agent: "planner", description: "Find the health endpoint", text: $text}')
# start_session HEADER: that start. Prints the HTTP status; the answer is $tmp/run.out.
start_session() {
  curl -sS --max-time 60 -o "$tmp/run.out" -w '%{http_code}' -X POST "$base/api/threads/$thread/sessions" -H "$1" \
    -H 'content-type: application/json' -d "$start" 2>"$tmp/err" || true
}
code=$(start_session "$signing")
expect "a helper start with no envelope is a 403" "$code" "403"
expect "  its code is prompt_signature_invalid (helper start)" "$(problem .code)" "prompt_signature_invalid"
expect "  its reason is invalid (helper start)" "$(problem .reason)" "invalid"
expect "the same start from dev-chat gets past the check, to a thread that does not exist" "$(start_session "$control")" "404"

# --- a signing client's prompt with a well-formed envelope -------------------------------------------------------------
echo "== a signing client (azp dev-signing) with a well-formed envelope"
envelope='eyJhbGciOiJFUzI1NiJ9..c2ln'
header_part='eyJhbGciOiJFUzI1NiJ9'
accepted="accepted-$(uuid)"
thread=$(uuid)
code=$(run_agui "$signing" "$thread" "add a health endpoint ($accepted)" "$envelope")
expect "a well-formed detached JWS is accepted" "$code" "200"
expect "the thread ends done" "$(wait_state "$thread" "$signing")" "done"
expect "mock-coder was sent the prompt exactly once" "$(sent "$accepted")" "1"
get "$signing" "/api/threads/$thread/export" >"$tmp/export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/export.json"
expect "the export holds the message (the checks below are not of an empty file)" \
  "$(jq -r --arg t "$accepted" '[.events[] | select(.kind == "user_message" and ((.data.text // "") | contains($t)))] | length' "$tmp/export.json" 2>/dev/null || echo unreadable)" "1"
expect "the envelope is in no export" "$(grep -c -F "$envelope" "$tmp/export.json" || true)" "0"
expect "the part that names the envelope's header is in no export" "$(grep -c -F "$header_part" "$tmp/export.json" || true)" "0"
expect "the export names no envelope member" "$(grep -E -c 'promptSignature|prompt_signature' "$tmp/export.json" || true)" "0"
expect "the envelope is in none of the run's own frames" "$(grep -c -F "$envelope" "$tmp/run.out" || true)" "0"
frames "$thread" "$signing" "$tmp/frames.json"
expect "the replay holds the message (the checks below are not of an empty stream)" \
  "$(grep -q -F "$accepted" "$tmp/frames.json" && echo yes || echo no)" "yes"
expect "the envelope is in no replayed frame" "$(grep -c -F "$envelope" "$tmp/frames.json" || true)" "0"
expect "the part that names the envelope's header is in no replayed frame" "$(grep -c -F "$header_part" "$tmp/frames.json" || true)" "0"

# --- the negative control: a client the list does not name -------------------------------------------------------------
echo "== a client that is not a signing client (azp dev-chat)"
control_nonce="control-$(uuid)"
thread=$(uuid)
code=$(run_agui "$control" "$thread" "add a health endpoint ($control_nonce)" "")
expect "a prompt from dev-chat with no envelope is accepted" "$code" "200"
expect "its thread ends done" "$(wait_state "$thread" "$control")" "done"
expect "mock-coder was sent it exactly once" "$(sent "$control_nonce")" "1"

finish
