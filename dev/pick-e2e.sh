#!/usr/bin/env sh
# System-level test of a message's model and reasoning effort (model-pick/v1 and adam's reasoning/v1, ADR 0086,
# docs/api/model-pick-v1.md): the agent list carries what the live card offers; a run that names a pick in
# `forwardedProps` is checked against the card, logged on the `user_message`, and sent to the agent under the extension's
# URI with the extension activated; a pick the card does not offer, or a value that is not one, is refused before
# anything is written, with the offered list on the typed 422.
#
#   dev/pick-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-pick` (dev/agents.yaml), a WireMock A2A agent (dev/wiremock/pick, ours): its card lists
# model-pick/v1 with the neutral aliases `alias-a` and `alias-b` (default `alias-a`) and reasoning/v1 with `low` and
# `high` (default `low`); a request that activates the extensions and names both is answered with both echoed on its
# task, its `working` update and its last words ("Worked with the pick: alias-b, high."); any other request is answered
# under its defaults. The plain agent is `mock-coder` (`mock-agent`), whose card lists neither extension.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /api/agents lists mock-pick with `pick` = {models: [alias-a, alias-b], modelDefault: alias-a, efforts:
#     [low, high], effortDefault: low} (what the web draws its chips from), and mock-coder with no `pick` at all;
#   * a run naming `alias-b` and `high` is accepted, its thread ends `done`; the log has the pick on the `user_message`
#     and the agent's words name it; mock-pick was sent both members in the message's metadata, under each URI, with the
#     `A2A-Extensions` header and `message.extensions` activating both;
#   * a second message of an open thread names only `alias-a`: it is sent with only the model activated, and a third
#     names none: it carries no member and activates nothing;
#   * `alias-c` and `high` (an alias the card does not offer), `low` and `max` (a level it does not offer) and a model
#     for mock-coder are 422 with `code` `model_not_offered` / `reasoning_not_offered` and `offered` naming what the card
#     does offer (empty for mock-coder): no thread, and the agent got nothing;
#   * `Alias B` is a 400, not an alias of the contract's alphabet.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_PICK_URL   http://127.0.0.1:${MOCK_PICK_PORT:-8103}   the agent that answers under a pick (WireMock admin API)
#   MOCK_CODER_URL  http://127.0.0.1:${MOCK_AGENT_PORT:-8081}  the plain agent (WireMock admin API)
#   TIMEOUT         120    seconds to wait for a thread to stop
#
# It EMPTIES the request journals of `mock-pick` and `mock-agent` first, so run it on a stack you are not in the middle
# of another scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml; the orchestrator side is also `picks.rs` of the app and the surface. The mock and this
# script's journal reader were run against a standalone WireMock 3.13.2 (not the stack: no orchestrator was started).
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
pick_agent=${MOCK_PICK_URL:-http://127.0.0.1:${MOCK_PICK_PORT:-8103}}
pick_agent=${pick_agent%/}
coder=${MOCK_CODER_URL:-http://127.0.0.1:${MOCK_AGENT_PORT:-8081}}
coder=${coder%/}
timeout=${TIMEOUT:-120}

model_uri=https://agents.vymalo.com/a2a/extensions/model-pick/v1
effort_uri=https://agents.vymalo.com/a2a/extensions/reasoning/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "pick e2e passed"; else echo "pick e2e FAILED"; exit 1; fi
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

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# run_agui AGENT THREAD TEXT [PROPS]: one run (one message) on THREAD, to its end, with forwardedProps PROPS ({ } by default).
# Prints the HTTP status; the body is $tmp/run.out (the stream, or the problem).
run_agui() {
  _props=${4:-'{}'}
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

# sent URL TEXT: for each SendStreamingMessage whose text holds TEXT, `<model or ->|<effort or ->|<model activated by the
# header>|<effort activated by the header>|<model in message.extensions>|<effort in message.extensions>`, one line each,
# oldest first (WireMock's journal, newest first: reversed; a header name in any case).
sent() {
  curl -s --max-time 30 "$1/__admin/requests" | jq -r --arg t "$2" --arg m "$model_uri" --arg e "$effort_uri" '
    def on($uri):
      [.headers | to_entries[] | select(.key | ascii_downcase == "a2a-extensions") | .value
       | if type == "array" then join(",") else tostring end] | join(",") | contains($uri);
    [.requests[] | .request as $r | ($r.body | fromjson? // {}) as $b
     | select($b.method == "SendStreamingMessage" and ([$b.params.message.parts[]?.text // ""] | join(" ") | contains($t)))
     | (($b.params.message.metadata[$m].model) // "-") + "|"
       + (($b.params.message.metadata[$e].effort) // "-") + "|"
       + (if ($r | on($m)) then "yes" else "no" end) + "|"
       + (if ($r | on($e)) then "yes" else "no" end) + "|"
       + (if (($b.params.message.extensions // []) | index($m)) != null then "yes" else "no" end) + "|"
       + (if (($b.params.message.extensions // []) | index($e)) != null then "yes" else "no" end)] | reverse | .[]' 2>/dev/null || true
}

reset_journal() { # reset_journal URL
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$1/__admin/requests" || true)
  if [ "$_code" = 200 ]; then ok "journal reset: $1"; else bad "journal reset: $1 answered HTTP $_code"; fi
}

# --- the stack ---------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
for a in mock-pick mock-coder; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done
ok "GET /api/agents lists mock-pick and mock-coder"
reset_journal "$pick_agent"
reset_journal "$coder"
nonce=$(uuid)

# --- what the web reads ------------------------------------------------------------------------------------------------
echo "== the agent list carries what the live card offers"
api GET /api/agents >"$tmp/agents.json" 2>/dev/null || echo '[]' >"$tmp/agents.json"
expect "mock-pick: pick is the two neutral aliases and the two levels, with the agent's own defaults" \
  "$(jq -cS '[.[] | select(.id == "mock-pick") | .pick][0]' "$tmp/agents.json")" \
  '{"efforts":["low","high"],"effortDefault":"low","modelDefault":"alias-a","models":[{"id":"alias-a","label":"Alias A"},{"id":"alias-b","label":"Alias B"}]}'
expect "mock-coder: its card lists neither extension, so there is no pick member and the web draws no chip" \
  "$(jq -r '[.[] | select(.id == "mock-coder")][0] | has("pick")' "$tmp/agents.json")" "false"

# --- a message that names a pick ---------------------------------------------------------------------------------------
echo "== mock-pick: alias-b and high, then alias-a alone, then none"
thread=$(uuid)
code=$(run_agui mock-pick "$thread" "fix the test, take your time ($nonce)" \
  '{"vymalo.model":"alias-b","vymalo.reasoningEffort":"high"}')
expect "POST /agui/agents/mock-pick naming alias-b and high answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/pick.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/pick.export.json"
expect "the log: the user_message holds the resolved pick" \
  "$(jq -cS '[.events[] | select(.kind == "user_message") | .data.pick // null] | first' "$tmp/pick.export.json")" \
  '{"model":"alias-b","reasoningEffort":"high"}'
expect "the log: the agent's last words name the alias and the level it was told" \
  "$(jq -r '[.events[] | select(.kind == "agent_status" and .data.status == "completed") | .data.detail] | first // ""' "$tmp/pick.export.json")" \
  "Worked with the pick: alias-b, high."
expect "mock-pick was sent both members in the message's metadata, under each URI, with the header and message.extensions" \
  "$(sent "$pick_agent" "$nonce" | paste -sd, -)" "alias-b|high|yes|yes|yes|yes"

# a second message of the same thread names only a model
code=$(run_agui mock-pick "$thread" "now the other one ($nonce)" '{"vymalo.model":"alias-a"}')
expect "a second message naming only alias-a answers 200" "$code" "200"
expect "the thread ends done again" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/two.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/two.export.json"
expect "the log: each message has its own pick, and the second names only the model" \
  "$(jq -cS '[.events[] | select(.kind == "user_message") | .data.pick // null]' "$tmp/two.export.json")" \
  '[{"model":"alias-b","reasoningEffort":"high"},{"model":"alias-a"}]'
expect "mock-pick was sent alias-b with high, then alias-a with no effort activated" \
  "$(sent "$pick_agent" "$nonce" | paste -sd, -)" "alias-b|high|yes|yes|yes|yes,alias-a|-|yes|no|yes|no"

# a third message names no pick: nothing is carried over
code=$(run_agui mock-pick "$thread" "and a last thing, no preference ($nonce)")
expect "a third message naming no pick answers 200" "$code" "200"
expect "the thread ends done a third time" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/three.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/three.export.json"
expect "the log: the third message names no pick (a pick is not carried over)" \
  "$(jq -cS '[.events[] | select(.kind == "user_message") | .data.pick // null]' "$tmp/three.export.json")" \
  '[{"model":"alias-b","reasoningEffort":"high"},{"model":"alias-a"},null]'
expect "mock-pick was sent no member and no activation for the third" \
  "$(sent "$pick_agent" "$nonce" | paste -sd, -)" "alias-b|high|yes|yes|yes|yes,alias-a|-|yes|no|yes|no,-|-|no|no|no|no"

# --- a pick that is refused --------------------------------------------------------------------------------------------
echo "== a pick the card does not offer is refused, with the offered list"
bad_model_thread=$(uuid)
code=$(run_agui mock-pick "$bad_model_thread" "deep please ($nonce)" '{"vymalo.model":"alias-c"}')
expect "an alias the card does not offer is a 422" "$code" "422"
expect "its code is model_not_offered" "$(jq -r '.code // ""' "$tmp/run.out")" "model_not_offered"
expect "it carries the aliases the card does offer" "$(jq -cS '.offered // []' "$tmp/run.out")" '["alias-a","alias-b"]'

bad_effort_thread=$(uuid)
code=$(run_agui mock-pick "$bad_effort_thread" "think hard please ($nonce)" '{"vymalo.reasoningEffort":"max"}')
expect "a level the card does not offer is a 422" "$code" "422"
expect "its code is reasoning_not_offered" "$(jq -r '.code // ""' "$tmp/run.out")" "reasoning_not_offered"
expect "it carries the levels the card does offer" "$(jq -cS '.offered // []' "$tmp/run.out")" '["low","high"]'

plain_thread=$(uuid)
code=$(run_agui mock-coder "$plain_thread" "add a health endpoint ($nonce)" '{"vymalo.model":"alias-a"}')
expect "a model for an agent whose card lists neither extension is a 422" "$code" "422"
expect "its code is model_not_offered" "$(jq -r '.code // ""' "$tmp/run.out")" "model_not_offered"
expect "it offers nothing, the card listing neither" "$(jq -cS '.offered // []' "$tmp/run.out")" '[]'

bad_value_thread=$(uuid)
code=$(run_agui mock-pick "$bad_value_thread" "the label, not the alias ($nonce)" '{"vymalo.model":"Alias B"}')
expect "a model that is not one of the contract's aliases is a 400" "$code" "400"
expect "its detail names the key and the alphabet" "$(jq -r '.detail // ""' "$tmp/run.out")" \
  "vymalo.model must be 1 to 64 characters of a-z, 0-9, '.', '_' and '-'"

for t in "$bad_model_thread" "$bad_effort_thread" "$plain_thread" "$bad_value_thread"; do
  expect "no thread was created for $t" "$(curl -s -o /dev/null -w '%{http_code}' -H "$id_header" "$base/api/threads/$t")" "404"
done
expect "mock-pick got no request for a refused pick" "$(sent "$pick_agent" "$nonce" | wc -l | tr -d ' ')" "3"
expect "mock-coder was sent nothing" "$(sent "$coder" "$nonce" | wc -l | tr -d ' ')" "0"

finish
