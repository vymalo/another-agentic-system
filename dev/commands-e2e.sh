#!/usr/bin/env sh
# System-level test of commands/v1 against the REAL coder (ADR 0079, docs/api/commands-v1.md): the card of adam-coder
# lists the prompt templates of its `commands/` folder, the orchestrator carries them in the capabilities document, and
# a message that starts with `/<name>` reaches the model expanded by the agent, which is where adam does it.
#
#   dev/commands-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * `GET /agui/agents/adam/capabilities` lists, under the commands/v1 URI, exactly the commands of the folder the coder
#     runs on (dev/coder/agent/commands, AGENT_DIR: `explain`, `fix-checks` and `review`), sorted by name, each with a
#     description; the coder's own card lists the extension;
#   * an agent whose card lists no commands lists none: `chat` (a folder with no `commands/`) and, when the stack has
#     it, the WireMock agent `mock-coder` have no such entry, and the extension is not in their capabilities at all;
#   * the person's message `/explain acme/app [mock:commands] <nonce>` is logged exactly as typed (the orchestrator expands nothing), and the coder's
#     FIRST model request, read from the request journal of mock-openai, holds the expanded template of
#     `commands/explain.md` with `acme/app` where `$ARGUMENTS` stands, and no user message that begins with `/explain`;
#   * `//explain acme/app` is the literal text `/explain acme/app` (adam-rs `crates/adam-a2a-runtime/src/commands.rs`:
#     `//<name>` for a command the agent serves drops one slash and expands nothing): the model's request holds it as
#     typed, with none of the template's words.
# The model is the `mock-coder` script `[mock:commands]` (dev/wiremock/coder-commands, ours): a message that holds the keyword
# gets one plain answer and the run ends, so the coder's own pull-request chain does not run. Each message holds a nonce, and the
# checks read the journal of mock-openai for the requests that hold it, the oldest being the first model request.
#
# Known gap: what the orchestrator sends to the coder over A2A is not asserted (the journal of the model is the earliest place
# this scenario reads); the log's user_message, which the orchestrator sends as it stands, is.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}
#   CODER_URL        http://127.0.0.1:${CODER_PORT:-8090}   the coder itself, for its public card
#   AGENT_DIR        dev/coder/agent   the folder the coder is running on (CODER_AGENT_DIR of compose.yaml is read too; a relative
#                    path is the repository's); its commands/ names are the expected list
#   TIMEOUT          120    seconds to wait for a run
#
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
openai=${MOCK_OPENAI_URL:-http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}}
openai=${openai%/}
coder_url=${CODER_URL:-http://127.0.0.1:${CODER_PORT:-8090}}
coder_url=${coder_url%/}
timeout=${TIMEOUT:-120}
commands_uri=https://agents.vymalo.com/a2a/extensions/commands/v1

root=$(cd "$(dirname "$0")/.." && pwd)
agent_dir=${AGENT_DIR:-${CODER_AGENT_DIR:-$root/dev/coder/agent}}
case $agent_dir in /*) ;; *) agent_dir=$root/$agent_dir ;; esac

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "commands e2e passed"; else echo "commands e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"; }
# shellcheck disable=SC1091 # the path is the script's own directory: shellcheck does not follow it
. "$(dirname "$0")/sse.sh"
uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# --- the commands of the folder -------------------------------------------------------------------------
if [ ! -d "$agent_dir/commands" ]; then
  echo "FAIL $agent_dir/commands does not exist (AGENT_DIR is the folder the coder runs on)"
  exit 1
fi
want=$(for f in "$agent_dir"/commands/*.md; do basename "$f" .md; done | LC_ALL=C sort | tr '\n' ' ')
want=${want% }
echo "the commands of $agent_dir: $want"
template=$(awk 'c >= 2 { print } /^---$/ { c++ }' "$agent_dir/commands/explain.md")
first_line=$(printf '%s\n' "$template" | sed -n '1p')
# The template's line that holds $ARGUMENTS, with the arguments put in: what the model must read.
args_line=$(printf '%s\n' "$template" | sed -n '/\$ARGUMENTS/{p;q;}')
if [ -z "$first_line" ] || [ -z "$args_line" ]; then
  echo "FAIL could not read the template, or its \$ARGUMENTS line, of $agent_dir/commands/explain.md"
  exit 1
fi
want_line=$(printf '%s' "$args_line" | sed 's|\$ARGUMENTS|acme/app|')

# --- the capabilities document --------------------------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  ids=$(printf '%s' "$agents" | jq -r '[.[].id] | join(" ")')
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") ${agents:-}"
  finish
fi
caps() { api GET "/agui/agents/$1/capabilities" 2>/dev/null || echo '{}'; }

# shellcheck disable=SC2016 # jq's own variables, not the shell's
got=$(caps adam | jq -r --arg u "$commands_uri" '[.custom[$u].commands[]?.name] | join(" ")')
if [ "$got" = "$want" ]; then ok "the capabilities of adam list the folder's commands, sorted: $got"; else bad "the capabilities of adam list '$got', want '$want'"; fi
# shellcheck disable=SC2016
if caps adam | jq -e --arg u "$commands_uri" '(.custom[$u].commands // []) | length > 0 and all(.[]; (.description | type == "string" and length > 0))' >/dev/null 2>&1; then
  ok "each command has a description"
else
  bad "a command of adam has no description"
fi
# shellcheck disable=SC2016
if caps adam | jq -e --arg u "$commands_uri" '.custom[$u].commands[] | select(.name == "explain") | .argumentHint | type == "string" and length > 0' >/dev/null 2>&1; then
  ok "explain carries its argument hint"
else
  bad "explain has no argumentHint in the capabilities of adam"
fi
# shellcheck disable=SC2016
if curl -fsS --max-time 30 "$coder_url/.well-known/agent-card.json" 2>/dev/null |
  jq -e --arg u "$commands_uri" '[.capabilities.extensions[]?.uri] | index($u) != null' >/dev/null 2>&1; then
  ok "the coder's own card lists commands/v1"
else
  bad "the coder's card at $coder_url does not list $commands_uri"
fi

for other in chat mock-coder; do
  case " $ids " in
    *" $other "*)
      # A real document first (an HTTP error or an empty answer would pass "has no entry"), then the absence.
      if doc=$(api GET "/agui/agents/$other/capabilities" 2>"$tmp/err"); then
        # shellcheck disable=SC2016
        if printf '%s' "$doc" | jq -e --arg u "$commands_uri" '(.identity.name | type == "string" and length > 0) and ((.custom // {}) | has($u) | not)' >/dev/null 2>&1; then
          ok "$other has a capabilities document (named) and it lists no commands"
        else
          bad "$other's capabilities are not a named document with no commands/v1 entry: $(printf '%s' "$doc" | head -c 300)"
        fi
      else
        bad "GET /agui/agents/$other/capabilities: $(head -c 300 "$tmp/err") $(printf '%s' "$doc" | head -c 300)"
      fi ;;
    *)
      if [ "$other" = chat ]; then bad "GET /api/agents does not list chat (it lists: $ids)"; else echo "skip $other is not in this stack"; fi ;;
  esac
done

# --- a message that names a command ---------------------------------------------------------------------
# send TEXT NONCE: runs a thread of adam with that one message (TEXT holds the nonce and the keyword of the stub), reads
# the run to its end, and leaves in $tmp/first.json the OLDEST model request of mock-coder whose messages hold the nonce:
# WireMock lists the newest request first, so the oldest is `last`.
send() {
  thread=$(uuid)
  input=$(jq -n --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/adam" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" 2>"$tmp/err" || true)
  if [ "$code" != 200 ]; then
    bad "POST /agui/agents/adam answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
    return 1
  fi
  sse_run_to_end "$base" "$thread" "$tmp/run.sse" "$id_header" "$timeout"
  # shellcheck disable=SC2016
  curl -s --max-time 30 "$openai/__admin/requests" |
    jq -c --arg n "$2" '[.requests[].request.body | fromjson? | select(.model == "mock-coder" and (.messages | tojson | contains($n)))] | last // empty' \
      >"$tmp/first.json" 2>/dev/null || true
  [ -s "$tmp/first.json" ] || { bad "mock-openai saw no mock-coder request that holds $2"; return 1; }
}
# the text of the user messages of the request in $tmp/first.json, joined
user_text() {
  jq -r '[.messages[] | select(.role == "user") | .content | if type == "array" then map(.text // "") | join("") else . end] | join("\n---\n")' "$tmp/first.json"
}

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$openai/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $openai"; else bad "journal reset: $openai answered HTTP $code"; fi

n1=n$(uuid | tr -d -)
typed="/explain acme/app [mock:commands] $n1"
if send "$typed" "$n1"; then
  echo "thread $thread"
  user=$(user_text)
  case $user in
    *"$want_line"*) ok "the first model request holds the template's line with the arguments: $want_line" ;;
    *) bad "the first model request does not hold \"$want_line\": $(printf '%s' "$user" | head -c 300)" ;;
  esac
  case $user in
    *"$first_line"*) ok "it holds the template's own words: $first_line" ;;
    *) bad "it does not hold the template's first line \"$first_line\"" ;;
  esac
  case $user in
    *'$ARGUMENTS'*) bad "the template's \$ARGUMENTS was left in the request" ;;
    *) ok "no \$ARGUMENTS is left in the request" ;;
  esac
  if jq -e '[.messages[] | select(.role == "user") | .content | if type == "array" then map(.text // "") | join("") else . end | startswith("/explain")] | any | not' "$tmp/first.json" >/dev/null 2>&1; then
    ok "no user message of the request begins with /explain: the raw command did not reach the model"
  else
    bad "a user message of the model's request still begins with /explain"
  fi
  # The orchestrator logs and sends the person's text as typed (docs/api/commands-v1.md): the expansion is the agent's.
  api GET "/api/threads/$thread/export" >"$tmp/export.json" 2>/dev/null || echo '{"events":[]}' >"$tmp/export.json"
  # shellcheck disable=SC2016
  if jq -e --arg t "$typed" '[.events[] | select(.kind == "user_message") | .data.text] == [$t]' "$tmp/export.json" >/dev/null 2>&1; then
    ok "the log holds one user_message, its text exactly as typed"
  else
    bad "the log's user_message is not exactly the typed text: $(jq -c '[.events[] | select(.kind == "user_message") | .data.text]' "$tmp/export.json" 2>/dev/null)"
  fi
  # shellcheck disable=SC2016
  if jq -e --arg l "$first_line" '[.events[] | tojson | contains($l)] | any | not' "$tmp/export.json" >/dev/null 2>&1; then
    ok "nothing of the expansion is in the log"
  else
    bad "the template's words are in the log: the expansion was written to it"
  fi
fi

n2=n$(uuid | tr -d -)
if send "//explain acme/app [mock:commands] $n2" "$n2"; then
  echo "thread $thread"
  user=$(user_text)
  case $user in
    "/explain acme/app [mock:commands] $n2"*) ok "//explain reaches the model as the literal text /explain acme/app" ;;
    *) bad "the model's request does not hold the literal '/explain acme/app ...': $(printf '%s' "$user" | head -c 300)" ;;
  esac
  case $user in
    *"//explain"*) bad "the second slash was not dropped: the request holds //explain" ;;
    *) ok "one slash was dropped" ;;
  esac
  case $user in
    *"$first_line"* | *"$want_line"*) bad "the escaped command was expanded" ;;
    *) ok "nothing of the template was expanded" ;;
  esac
fi

# The journal was emptied above and the stub answers both messages, so nothing may be off-script.
unmatched=$(curl -s --max-time 30 "$openai/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then
  ok "mock-openai matched every request"
else
  bad "mock-openai saw $unmatched unmatched requests (see $openai/__admin/requests/unmatched)"
fi

finish
