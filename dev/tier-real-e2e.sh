#!/usr/bin/env sh
# The access tier (access-tier/v1, ADR 0061, docs/api/access-tier-v1.md) against the REAL coder, adam-coder on the pinned image: its card
# lists the tiers up to the deployment's cap, a message at `plan` makes it refuse a write it makes at `edits`, and a tier above the cap is
# refused by the orchestrator before anything is written. dev/tier-e2e.sh proves the orchestrator's half against a WireMock agent; this is
# the two halves meeting.
#
#   dev/tier-real-e2e.sh
#
# Start the `app` profile first (the coder image is about 3 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `coder-share` (dev/agents.yaml): the coder itself with no verification gate, since nothing here is pushed. Its model is
# `mock-coder` on `mock-openai`, and the keyword `[mock:tier]` selects the script (dev/wiremock/coder-tier, ours, not vendored; each turn
# has an SSE twin and `dev/check-agent-mocks.sh` plays it): `start_scratch` (tier-call-1), `write_file` note.txt (tier-call-2), then an
# answer. The script is the same whatever the tier, on purpose: the model is a script and offers nothing of its own, so what differs
# between the two runs is what the coder does with the same two calls. adam-rs (bin/adam-coder/src/tier.rs) holds a tool above the tier
# back at two places: the model is not offered it (`TierSource::withhold`: it is missing from the `tools` of the request) and a
# call to it is refused when it would run, with a result that says `` `write_file` needs the edits access tier ``. `start_scratch` and
# `write_file` need `edits`, so at `plan` both are refused.
#
# The cap. The coder lists the tiers up to `MAX_ACCESS_TIER`, which compose.yaml does not set: the default is `auto`, so the card lists
# `plan`, `edits`, `auto` (default `auto`) and `bypass`, opt-in, is not there.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/agents lists coder-share; GET /agui/agents/coder-share/capabilities has `custom[<uri>] = {tiers: [plan, edits, auto],
#     default: auto}`;
#   * a run naming `plan` answers 200 and its thread ends `done`; the log has the tier on the `user_message` and one `agent_status` that
#     the coder echoed `plan` on; the model's first request was not offered `write_file`, and its last one, after the two calls, carries
#     the coder's refusal of `write_file` (and of `start_scratch`) as the tool results;
#   * a run naming `edits`, same script, same words: the thread ends `done`, the echo is `edits`, `write_file` was offered, and no tool
#     result of the run says an access tier refused anything, and the write_file result says `Created note.txt`;
#   * `bypass`, above the cap, is a 422 ("this agent does not take the tier 'bypass'"): no thread, and the model was sent nothing.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}   the coder's model (WireMock admin API)
#   TIMEOUT          120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-openai` first, so run it on a stack you are not in the middle of another scenario on. Needs
# curl and jq (and python3, or /proc, uuidgen for a v7 UUID). Verified by CI only, in .github/workflows/coder-e2e.yml, through dev/e2e-all.sh.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
openai=${MOCK_OPENAI_URL:-http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}}
openai=${openai%/}
timeout=${TIMEOUT:-120}

tier_uri=https://agents.vymalo.com/a2a/extensions/access-tier/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "tier real e2e passed"; else echo "tier real e2e FAILED"; exit 1; fi
}
expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { python3 -c 'import os,time;m=time.time_ns()//1000000;print("%08x-%04x-7%03x-%04x-%012x"%(m>>16,m&65535,int.from_bytes(os.urandom(2),"big")&4095,32768|int.from_bytes(os.urandom(2),"big")&16383,int.from_bytes(os.urandom(6),"big")))' 2>/dev/null || cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# run_agui AGENT THREAD TEXT TIER: one run (one message) on THREAD, naming TIER. Prints the HTTP status; the body is $tmp/run.out.
run_agui() {
  _props=$(jq -n --arg t "$4" '{"vymalo.tier": $t}')
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" --argjson props "$_props" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.out" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

# wait_state THREAD: waits (up to TIMEOUT seconds) until the thread stops, and prints the state it ended in (or the last one it saw).
wait_state() {
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(api GET "/api/threads/$1" 2>/dev/null | jq -r '.state // empty' || true)
    case $_state in done | blocked | failed | cancelled) echo "$_state"; return 0 ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then echo "${_state:-unknown}"; return 0; fi
    sleep 2
  done
}

# requests_of NONCE: the model requests of that run, oldest first, one JSON object each (WireMock's journal is newest first).
requests_of() {
  curl -s --max-time 30 "$openai/__admin/requests" | jq -c --arg n "$1" '
    [.requests[] | (.request.body | fromjson? // {})
     | select(((.messages // []) | tostring) | contains($n))] | reverse | .[]' 2>/dev/null || true
}

# offered REQUEST TOOL: true when the request's `tools` offers TOOL to the model.
offered() {
  printf '%s' "$1" | jq -r --arg t "$2" '[(.tools // [])[] | .function.name] | index($t) != null' 2>/dev/null || echo unknown
}

# tool_results REQUEST: what the tool messages of the request say, joined.
tool_results() {
  printf '%s' "$1" | jq -r '[(.messages // [])[] | select(.role == "tool") | (.content | if type == "string" then . else tostring end)] | join(" || ")' 2>/dev/null || true
}

# --- the stack ---------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" coder-share "*) ok "GET /api/agents lists coder-share" ;;
  *) echo "the agent 'coder-share' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$openai/__admin/requests" || true)
expect "the coder's model journal was emptied" "$code" "200"

# --- what the coder's card says ----------------------------------------------------------------------------------------
echo "== the capabilities: the tiers up to the coder's cap"
expect "coder-share: custom[access-tier/v1] lists plan, edits and auto (no MAX_ACCESS_TIER: the cap is auto, bypass is opt-in), default auto" \
  "$(curl -sS --max-time 30 -H "$id_header" "$base/agui/agents/coder-share/capabilities" | jq -cS --arg u "$tier_uri" '.custom[$u]')" \
  '{"default":"auto","tiers":["plan","edits","auto"]}'

# --- a message at plan: the coder refuses the write ----------------------------------------------------------------------
echo "== plan: the same two calls, refused"
plan_nonce=$(uuid)
plan_thread=$(uuid)
code=$(run_agui coder-share "$plan_thread" "[mock:tier] write note.txt ($plan_nonce)" plan)
expect "POST /agui/agents/coder-share naming plan answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$plan_thread")" "done"
api GET "/api/threads/$plan_thread/export" >"$tmp/plan.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/plan.export.json"
expect "the log: the user_message holds the tier" \
  "$(jq -r '[.events[] | select(.kind == "user_message") | .data.tier // "-"] | first // "-"' "$tmp/plan.export.json")" "plan"
expect "the log: the coder's echo of the tier is on its agent_status" \
  "$(jq -r '[.events[] | select(.kind == "agent_status" and (.data | has("tier"))) | .data.tier] | join(",")' "$tmp/plan.export.json")" "plan"
plan_first=$(requests_of "$plan_nonce" | head -1)
plan_last=$(requests_of "$plan_nonce" | tail -1)
expect "the model was called for that run" "$([ -n "$plan_first" ] && echo yes || echo no)" "yes"
expect "the model's first request was not offered write_file (held back at plan)" "$(offered "$plan_first" write_file)" "false"
expect "the model's first request was offered a read tool (the tier narrows, it does not empty the list)" "$(offered "$plan_first" read_file)" "true"
plan_results=$(tool_results "$plan_last")
# shellcheck disable=SC2016 # the backticks are literal: adam's refusal text quotes the tool's name
case $plan_results in
  *"\`write_file\` needs the edits access tier"*) ok "the coder refused the write_file call: its result says it needs the edits tier" ;;
  *) bad "the last model request does not carry the refusal of write_file as a tool result: ${plan_results:-no tool result}" ;;
esac
# shellcheck disable=SC2016 # the backticks are literal: adam's refusal text quotes the tool's name
case $plan_results in
  *"\`start_scratch\` needs the edits access tier"*) ok "the coder refused start_scratch too" ;;
  *) bad "the last model request does not carry the refusal of start_scratch as a tool result: ${plan_results:-no tool result}" ;;
esac

# --- the same words at edits: the coder writes --------------------------------------------------------------------------
echo "== edits: the same two calls, allowed"
edits_nonce=$(uuid)
edits_thread=$(uuid)
code=$(run_agui coder-share "$edits_thread" "[mock:tier] write note.txt ($edits_nonce)" edits)
expect "POST /agui/agents/coder-share naming edits answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$edits_thread")" "done"
api GET "/api/threads/$edits_thread/export" >"$tmp/edits.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/edits.export.json"
expect "the log: the user_message holds the tier" \
  "$(jq -r '[.events[] | select(.kind == "user_message") | .data.tier // "-"] | first // "-"' "$tmp/edits.export.json")" "edits"
expect "the log: the coder's echo of the tier is on its agent_status" \
  "$(jq -r '[.events[] | select(.kind == "agent_status" and (.data | has("tier"))) | .data.tier] | join(",")' "$tmp/edits.export.json")" "edits"
edits_first=$(requests_of "$edits_nonce" | head -1)
edits_last=$(requests_of "$edits_nonce" | tail -1)
expect "the model was called for that run" "$([ -n "$edits_first" ] && echo yes || echo no)" "yes"
expect "the model's first request was offered write_file" "$(offered "$edits_first" write_file)" "true"
edits_results=$(tool_results "$edits_last")
expect "the model's last request carries two tool results" \
  "$(printf '%s' "$edits_last" | jq -r '[(.messages // [])[] | select(.role == "tool")] | length')" "2"
case $edits_results in
  *'Created note.txt'*) ok "the write happened: the write_file result says Created note.txt" ;;
  *) bad "the edits run's tool results do not say Created note.txt: ${edits_results:-no tool result}" ;;
esac
case $edits_results in
  *'access tier'*) bad "a tool result of the edits run mentions an access tier: $edits_results" ;;
  *) ok "no tool result of the edits run says an access tier refused anything" ;;
esac

# --- a tier above the coder's cap ----------------------------------------------------------------------------------------
echo "== bypass: above the cap, refused before anything is written"
over_nonce=$(uuid)
over_thread=$(uuid)
code=$(run_agui coder-share "$over_thread" "[mock:tier] write note.txt ($over_nonce)" bypass)
expect "a tier above the coder's cap is a 422" "$code" "422"
expect "its detail says the agent does not take it" "$(jq -r '.detail // ""' "$tmp/run.out")" "this agent does not take the tier 'bypass'"
expect "no thread was created" "$(curl -s -o /dev/null -w '%{http_code}' -H "$id_header" "$base/api/threads/$over_thread")" "404"
# A side observation: the 422 and the 404 above are the proof; this only says the model was not reached either.
expect "(side observation) the coder's model saw no request for it" "$(requests_of "$over_nonce" | wc -l | tr -d ' ')" "0"

finish
