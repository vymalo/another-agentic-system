#!/usr/bin/env sh
# System-level test of the interface locale (locale/v1, ADR 0085, docs/api/locale-v1.md): a message that names a BCP 47 tag is sent to an agent
# whose card lists the extension, with the extension activated and the tag in the message's metadata; a message that names none activates
# nothing and carries no member at all; a tag that is not a tag is refused before anything is written; an agent whose card does not list the
# extension gets the plain message it always got; and nothing of the tag is in the log or in the AG-UI stream.
#
#   dev/locale-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-locale` (dev/agents.yaml), a WireMock A2A agent (dev/wiremock/locale, ours): its card lists locale/v1 with `en-GB` as its
# own language; a request that activates the extension and names a tag is answered with that tag in its words ("The interface reads in fr-FR,
# so I answer in it."); any other request is answered in its own. The plain agent is `mock-coder` (`mock-agent`), whose card does not list the
# extension.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * GET /agui/agents/mock-locale/capabilities has no custom[<uri>]: nothing in the interface draws a locale control;
#   * a run on mock-locale naming `fr-FR` is accepted, its thread ends `done`, the agent's last words name fr-FR, mock-locale was sent the tag in
#     the message's metadata under the URI with the header and message.extensions, and NOTHING of it is in the log (the user_message has no
#     `locale`, the export holds none) nor in the AG-UI stream (no `vymalo.locale`, no activity);
#   * a tag is carried as the client wrote it: `FR-FR` reaches the agent as `FR-FR`;
#   * a second message of the thread naming none activates nothing and carries no member at all;
#   * `en_CM` and `klingon please` are 400 and write nothing;
#   * `fr-FR` to mock-coder, whose card does not list the extension, is accepted (a hint never holds a message back) and mock-coder is sent the
#     plain message.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_LOCALE_URL  http://127.0.0.1:${MOCK_LOCALE_PORT:-8101}  the agent that reads the tag (WireMock admin API)
#   MOCK_CODER_URL   http://127.0.0.1:${MOCK_AGENT_PORT:-8081}   the plain agent (WireMock admin API)
#   TIMEOUT          120    seconds to wait for a thread to stop
#
# It EMPTIES the request journals of `mock-locale` and `mock-agent` first, so run it on a stack you are not in the middle of another scenario on.
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
localed=${MOCK_LOCALE_URL:-http://127.0.0.1:${MOCK_LOCALE_PORT:-8101}}
localed=${localed%/}
coder=${MOCK_CODER_URL:-http://127.0.0.1:${MOCK_AGENT_PORT:-8081}}
coder=${coder%/}
timeout=${TIMEOUT:-120}

locale_uri=https://agents.vymalo.com/a2a/extensions/locale/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "locale e2e passed"; else echo "locale e2e FAILED"; exit 1; fi
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

# run_agui AGENT THREAD TEXT [LOCALE]: one run (one message) on THREAD, to its end, naming LOCALE when it is given (a tag, or `null`).
# Prints the HTTP status; the body is $tmp/run.out (the stream, or the problem).
run_agui() {
  _props='{}'
  if [ "${4:-}" != "" ]; then _props=$(jq -n --arg l "$4" 'if $l == "null" then {"vymalo.locale": null} else {"vymalo.locale": $l} end'); fi
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

# frames THREAD FILE: the thread's AG-UI events, replayed from the start, as one JSON array in FILE.
frames() {
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' "$base/agui/threads/$1/connect?mode=run" 2>/dev/null |
    sed -n 's/^data: *//p' | jq -s '.' >"$2" 2>/dev/null || echo '[]' >"$2"
}

# sent URL TEXT: for each SendStreamingMessage whose text holds TEXT, `<tag or ->|<activated by the header: yes or no>|<by the message:
# yes or no>|<the URI is a key of the metadata: yes or no>`, one line each, oldest first (WireMock's journal, newest first: reversed; a header
# name in any case).
sent() {
  curl -s --max-time 30 "$1/__admin/requests" | jq -r --arg t "$2" --arg u "$locale_uri" '
    [.requests[] | .request as $r | ($r.body | fromjson? // {}) as $b
     | select($b.method == "SendStreamingMessage" and ([$b.params.message.parts[]?.text // ""] | join(" ") | contains($t)))
     | ((($b.params.message.metadata // {})[$u].locale) // "-") + "|"
       + (if ([$r.headers | to_entries[] | select(.key | ascii_downcase == "a2a-extensions") | .value
              | if type == "array" then join(",") else tostring end] | join(",") | contains($u)) then "yes" else "no" end) + "|"
       + (if (($b.params.message.extensions // []) | index($u)) != null then "yes" else "no" end) + "|"
       + (if (($b.params.message.metadata // {}) | has($u)) then "yes" else "no" end)] | reverse | .[]' 2>/dev/null || true
}

reset_journal() { # reset_journal URL
  _code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$1/__admin/requests" || true)
  if [ "$_code" = 200 ]; then ok "journal reset: $1"; else bad "journal reset: $1 answered HTTP $_code"; fi
}

# with_a_locale FILE: how many objects of FILE hold a `locale` member (1 is what a hint leaves: the user_message's data).
with_a_locale() { jq '[.. | objects | select(has("locale"))] | length' "$1" 2>/dev/null || echo unknown; }

# --- the stack ---------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
for a in mock-locale mock-coder; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done
ok "GET /api/agents lists mock-locale and mock-coder"
reset_journal "$localed"
reset_journal "$coder"
nonce=$(uuid)

# --- what the interface reads ------------------------------------------------------------------------------------------
echo "== the capabilities document says nothing new"
caps() { curl -sS --max-time 30 -H "$id_header" "$base/agui/agents/$1/capabilities"; }
expect "mock-locale: no custom[locale/v1] (nothing draws a locale control)" \
  "$(caps mock-locale | jq -r --arg u "$locale_uri" '(.custom // {}) | has($u)')" "false"

# --- a message that names a locale -------------------------------------------------------------------------------------
echo "== mock-locale: fr-FR"
thread=$(uuid)
code=$(run_agui mock-locale "$thread" "corrige la page de connexion ($nonce)" fr-FR)
expect "POST /agui/agents/mock-locale naming fr-FR answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$thread")" "done"
api GET "/api/threads/$thread/export" >"$tmp/locale.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/locale.export.json"
expect "the log: the agent's last words name the tag it read" \
  "$(jq -r '[.events[] | select(.kind == "agent_status" and .data.status == "completed") | .data.detail] | first // ""' "$tmp/locale.export.json")" \
  "The interface reads in fr-FR, so I answer in it."
expect "the log: the user_message records the tag (the log is the state)" \
  "$(jq -r '[.events[] | select(.kind == "user_message") | .data.locale // "-"] | first // "-"' "$tmp/locale.export.json")" "fr-FR"
expect "the log: the tag is on the user_message and nowhere else" "$(with_a_locale "$tmp/locale.export.json")" "1"
frames "$thread" "$tmp/locale.frames.json"
expect "AG-UI: the user message's START carries no vymalo.locale" \
  "$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "user") | .metadata["vymalo.locale"] // "-"] | first // "-"' "$tmp/locale.frames.json")" "-"
expect "AG-UI: the user message's START is there, with its actor (the stream is not empty)" \
  "$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "user") | (.metadata["vymalo.actor"] != null)] | first // "absent"' "$tmp/locale.frames.json")" "true"
expect "AG-UI: no activity is emitted for a locale" \
  "$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT") | (.activityType // "")] | map(select(contains("locale"))) | length' "$tmp/locale.frames.json")" "0"
expect "mock-locale was sent the tag in the message's metadata, with the header and message.extensions" \
  "$(sent "$localed" "$nonce" | paste -sd, -)" "fr-FR|yes|yes|yes"

# a tag is carried as the client wrote it, and its subtags are matched case-insensitively
code=$(run_agui mock-locale "$thread" "et la redirection ? ($nonce)" FR-FR)
expect "a second message naming FR-FR answers 200" "$code" "200"
expect "the thread ends done again" "$(wait_state "$thread")" "done"
expect "mock-locale was sent the tag as it was written" "$(sent "$localed" "$nonce" | paste -sd, -)" \
  "fr-FR|yes|yes|yes,FR-FR|yes|yes|yes"

# a message that names none activates nothing and carries no member at all
code=$(run_agui mock-locale "$thread" "and one more thing ($nonce)")
expect "a third message naming no locale answers 200" "$code" "200"
expect "the thread ends done a third time" "$(wait_state "$thread")" "done"
expect "mock-locale was sent no tag and no activation, and the metadata has no member for the URI" \
  "$(sent "$localed" "$nonce" | paste -sd, -)" "fr-FR|yes|yes|yes,FR-FR|yes|yes|yes,-|no|no|no"
api GET "/api/threads/$thread/export" >"$tmp/three.export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/three.export.json"
expect "the log: the three messages record two tags and no member for the one that names none" \
  "$(with_a_locale "$tmp/three.export.json")" "2"
expect "the log: the message that named none has no locale member" \
  "$(jq -r '[.events[] | select(.kind == "user_message") | (.data | has("locale"))] | join(",")' "$tmp/three.export.json")" "true,true,false"

# --- a tag that is refused ---------------------------------------------------------------------------------------------
echo "== a string that is not a tag is refused and writes nothing"
bad_thread=$(uuid)
code=$(run_agui mock-locale "$bad_thread" "fix the test ($nonce)" en_CM)
expect "en_CM is a 400" "$code" "400"
expect "its detail names the key and an example" "$(jq -r '.detail // ""' "$tmp/run.out")" \
  "vymalo.locale must be a BCP 47 language tag, for example en-GB"
word_thread=$(uuid)
code=$(run_agui mock-locale "$word_thread" "fix the test ($nonce)" "klingon please")
expect "a word that is not a tag is a 400" "$code" "400"
for t in "$bad_thread" "$word_thread"; do
  expect "no thread was created for $t" "$(curl -s -o /dev/null -w '%{http_code}' -H "$id_header" "$base/api/threads/$t")" "404"
done
expect "mock-locale was sent nothing for a refused tag" "$(sent "$localed" "$nonce" | wc -l | tr -d ' ')" "3"

# --- an agent that does not list the extension -------------------------------------------------------------------------
echo "== a card without the extension gets the plain message"
plain_thread=$(uuid)
code=$(run_agui mock-coder "$plain_thread" "add a health endpoint ($nonce)" fr-FR)
expect "a tag for an agent whose card does not list the extension is accepted (a hint never holds a message back)" "$code" "200"
expect "its thread ends done" "$(wait_state "$plain_thread")" "done"
expect "mock-coder was sent the plain message: no tag, no activation, no member" \
  "$(sent "$coder" "$nonce" | paste -sd, -)" "-|no|no|no"

finish
