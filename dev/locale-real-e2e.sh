#!/usr/bin/env sh
# The interface language on a REAL adam agent (locale/v1, ADR 0069 / ADR 0085), end to end: the chat's card
# lists the extension with the deployment's own tag, a message that names a tag reaches adam, and adam tells its
# model the language in one more turn after the history — never in the stable prefix (`system`), which is what the
# prompt cache keeps (adam-rs ADR 0044). A tag for an agent whose card does not list the extension is accepted and
# changes nothing, and a thread that names no tag gets no line at all.
#
#   dev/locale-real-e2e.sh
#
# Start the `app` profile first:
#
#   docker compose --profile app up -d --build --wait
#
# It needs a chat image at or after adam-rs 945208ef (the `interface_language_turn` of
# https://github.com/vymalo/another-adam-rs/pull/347) and `DEFAULT_LOCALE: en-GB` on the `chat` service of
# compose.yaml: without either, the card lists nothing and the first check fails — which is the red proof this
# scenario was written from.
#
# The two agents are the real folder agents of the stack (`chat`, `researcher`: adam-agent on the pinned image, on
# a scripted model), so the model's own requests are read from `mock-model`'s WireMock journal.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the chat's card lists locale/v1 with `params.locale = en-GB`; the researcher's card lists no such extension;
#   * a message to the chat naming `de-DE` is answered, and the model's request for that turn has the line
#     "The person's interface language is de-DE" as one of its messages, and `de-DE` nowhere in `system`;
#   * a second thread of the chat naming no tag sends the model no such line;
#   * a tag for the researcher, whose card lists no extension, is accepted (a hint never holds a message back),
#     and its model request has no line either.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   CHAT_URL        http://127.0.0.1:${CHAT_PORT:-8097}        the chat agent (its card, and its token)
#   RESEARCHER_URL  http://127.0.0.1:${RESEARCHER_PORT:-8098} the agent whose card lists no locale
#   CHAT_TOKEN      dev-chat-token        CHAT_A2A_TOKEN of the chat service
#   RESEARCHER_TOKEN dev-researcher-token RESEARCHER_A2A_TOKEN of the researcher
#   MODEL_URL       http://127.0.0.1:${MOCK_MODEL_PORT:-8094}  the scripted model (WireMock admin API)
#   DEFAULT_TAG     en-GB                 the tag compose.yaml gives the chat as DEFAULT_LOCALE
#   TIMEOUT         120    seconds to wait for a thread to stop
#
# It EMPTIES the request journal of `mock-model` first, so run it on a stack you are not in the middle of another
# scenario on. Needs curl and jq (and /proc or uuidgen for a UUID). CI runs it in .github/workflows/coder-e2e.yml,
# through dev/e2e-all.sh.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
chated=${CHAT_URL:-http://127.0.0.1:${CHAT_PORT:-8097}}
chated=${chated%/}
chat_token=${CHAT_TOKEN:-dev-chat-token}
researched=${RESEARCHER_URL:-http://127.0.0.1:${RESEARCHER_PORT:-8098}}
researched=${researched%/}
researcher_token=${RESEARCHER_TOKEN:-dev-researcher-token}
modeled=${MODEL_URL:-http://127.0.0.1:${MOCK_MODEL_PORT:-8094}}
modeled=${modeled%/}
default_tag=${DEFAULT_TAG:-en-GB}
timeout=${TIMEOUT:-120}

locale_uri=https://agents.vymalo.com/a2a/extensions/locale/v1

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "locale real e2e passed"; else echo "locale real e2e FAILED"; exit 1; fi
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

card() { # card URL TOKEN: the agent's card
  curl --fail-with-body -sS --max-time 30 "$1/.well-known/agent-card.json" -H "authorization: Bearer $2"
}

# run_agui AGENT THREAD TEXT [LOCALE]: one run (one message) on THREAD, to its end, naming LOCALE when it is given.
# Prints the HTTP status; the body is $tmp/run.out.
run_agui() {
  _props='{}'
  if [ "${4:-}" != "" ]; then _props=$(jq -n --arg l "$4" '{"vymalo.locale": $l}'); fi
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" --argjson props "$_props" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: $props}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.out" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>/dev/null || true
}

# wait_state THREAD: waits (up to TIMEOUT seconds) until the thread stops, and prints the state it ended in.
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

# requests_of NONCE: the model requests of that run, oldest first, one JSON object each (the body of the call).
requests_of() {
  curl -s --max-time 30 "$modeled/__admin/requests" | jq -c --arg n "$1" '
    [.requests[] | (.request.body | fromjson? // {})
     | select(((.messages // []) | tostring) | contains($n))] | reverse | .[]' 2>/dev/null || true
}

# has_line REQUEST TAG: yes when one of the request's messages is the interface-language turn naming TAG.
has_line() {
  printf '%s' "$1" | jq -r --arg t "$2" \
    '(((.messages // []) | tostring) | contains("interface language is " + $t))' 2>/dev/null || echo false
}

# tag_in_system REQUEST TAG: yes when the tag leaked into the stable prefix.
tag_in_system() {
  printf '%s' "$1" | jq -r --arg t "$2" '(((.system // "") | tostring) | contains($t))' 2>/dev/null || echo unknown
}

# --- the cards ---------------------------------------------------------------------------------------------------------
echo "== the chat's card lists the extension with the deployment's tag"
chat_card=$(card "$chated" "$chat_token" 2>"$tmp/err" || true)
if [ -z "$chat_card" ]; then
  bad "the chat's card was not read from $chated/.well-known/agent-card.json: $(cat "$tmp/err")"
else
  ok "the chat's card was read"
fi
expect "the chat's card carries params.locale = $default_tag" \
  "$(printf '%s' "$chat_card" | jq -r --arg u "$locale_uri" '[.. | objects | select((.uri? // "") == $u) | (.params.locale? // "-")] | first // "-"')" \
  "$default_tag"
expect "the chat's card lists the extension once" \
  "$(printf '%s' "$chat_card" | jq -r --arg u "$locale_uri" '[.. | objects | select((.uri? // "") == $u)] | length')" "1"
echo "== an agent whose card does not list it"
researcher_card=$(card "$researched" "$researcher_token" 2>"$tmp/err" || true)
expect "the researcher's card does not list locale/v1" \
  "$(printf '%s' "$researcher_card" | jq -r --arg u "$locale_uri" '[.. | objects | select((.uri? // "") == $u)] | length')" "0"

echo "== the model's journal"
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$modeled/__admin/requests" || true)
expect "the model's journal was emptied" "$code" "200"

# --- a message that names a tag, on the real chat ----------------------------------------------------------------
echo "== the chat, naming de-DE"
nonce=$(uuid)
thread=$(uuid)
code=$(run_agui chat "$thread" "corrige la page de connexion ($nonce)" de-DE)
expect "a message to the chat naming de-DE answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$thread")" "done"
first=$(requests_of "$nonce" | head -1)
expect "the model was called for that turn" "$([ -n "$first" ] && echo yes || echo no)" "yes"
expect "the model's request carries the interface-language turn naming de-DE" "$(has_line "$first" de-DE)" "true"
expect "the tag is NOT in the stable prefix (the prompt cache keeps it)" "$(tag_in_system "$first" de-DE)" "false"

# --- a thread that names none ------------------------------------------------------------------------------------
echo "== a thread that names no tag"
plain_nonce=$(uuid)
plain_thread=$(uuid)
code=$(run_agui chat "$plain_thread" "et la redirection ? ($plain_nonce)")
expect "a message naming no tag answers 200" "$code" "200"
expect "the thread ends done" "$(wait_state "$plain_thread")" "done"
plain=$(requests_of "$plain_nonce" | head -1)
expect "the model was called for that turn" "$([ -n "$plain" ] && echo yes || echo no)" "yes"
expect "its request carries no interface-language turn at all" \
  "$(printf '%s' "$plain" | jq -r '(((.messages // []) | tostring) | contains("interface language is "))')" "false"

# --- an agent whose card lists no extension ----------------------------------------------------------------------
echo "== the researcher, whose card lists no locale"
res_nonce=$(uuid)
res_thread=$(uuid)
code=$(run_agui researcher "$res_thread" "résume cette page ($res_nonce)" de-DE)
expect "a tag for an agent without the extension is accepted (a hint never holds a message back)" "$code" "200"
expect "its thread ends done" "$(wait_state "$res_thread")" "done"
res=$(requests_of "$res_nonce" | head -1)
expect "the researcher's model was called" "$([ -n "$res" ] && echo yes || echo no)" "yes"
expect "its request carries no interface-language turn" \
  "$(printf '%s' "$res" | jq -r '(((.messages // []) | tostring) | contains("interface language is "))')" "false"

finish
