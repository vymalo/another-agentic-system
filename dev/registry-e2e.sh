#!/usr/bin/env sh
# System-level test of the platform's agent registry (MVP slice 9, ADR 0022): the orchestrator reads the agents the
# platform provisions live, beside its own, and says so when it cannot.
#
#   dev/registry-e2e.sh
#
# Start the `app` profile first (the registry is `mock-registry`, a WireMock stub of the default profile; the agents it
# lists are `mock-agent-releases` and `mock-agent`, also WireMock):
#
#   docker compose --profile app up -d --build --wait
#
# The registry document is `agent-registry/v1` of another-agentic-platform (docs/extensions/agent-registry-v1.md there):
# a linkset of agent cards, here dev/wiremock/registry, which lists `platform-coder` at the card of `mock-agent-releases`.
# The script changes what `mock-registry` answers through WireMock's admin API (POST /__admin/mappings, then
# POST /__admin/mappings/reset), and asks the orchestrator through the edge, as the UI does.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   (a) the registry's agent is listed: GET /api/agents lists the agents of dev/agents.yaml first, in their order, then
#       `platform-coder` with `source: registry`, its title and its tags; its `releases` are those of ITS CARD
#       (production, staging, latest), never the registry's; GET /api/registry says both sources are `ok`; a thread on
#       `platform-coder` is delegated and ends `done`, and the mock agent saw the bearer token
#       AGENT_REGISTRY_AGENT_TOKEN of compose.yaml (the deployment-wide credential of the registry's agents);
#   (b) an agent added to the registry shows up without a restart: a stub of `mock-registry` that lists `platform-helper`
#       beside `platform-coder` is in GET /api/agents within 10 s, after the static agents and `platform-coder`;
#   (c) a registry that is down leaves the static agents and says so: with the registry answering 503, within 10 s
#       GET /api/agents lists exactly the agents of dev/agents.yaml (none with `source: registry`), GET /api/registry says
#       `platform` is `unavailable`, a run on `platform-coder` is a 503 with Retry-After (never a 404), and a thread on a
#       static agent still ends `done`;
#   (d) the registry is read again when it answers: after POST /__admin/mappings/reset, within 10 s `platform-coder` is
#       back, `platform-helper` is gone, and the sources are `ok`.
# The registry is left as it was found (reset). Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL              http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL            dev@example.com, the user: the token of dev/auth-header.sh is theirs
#   MOCK_REGISTRY_URL     http://127.0.0.1:${MOCK_REGISTRY_PORT:-8084}, the registry's WireMock (its admin API)
#   MOCK_RELEASES_URL     http://127.0.0.1:${MOCK_AGENT_RELEASES_PORT:-8082}, the WireMock that is `platform-coder`
#   TIMEOUT               60     seconds to wait for a thread to stop
#   AGENT_TOKEN           dev-registry-agent-token   what compose.yaml sets AGENT_REGISTRY_AGENT_TOKEN to
#
# It RESETS the stubs of `mock-registry` first (a stub left by an earlier run would be in the way) and EMPTIES the
# request journal of `mock-agent-releases`, so run it on a stack you are not in the middle of another scenario on.
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
registry=${MOCK_REGISTRY_URL:-http://127.0.0.1:${MOCK_REGISTRY_PORT:-8084}}
registry=${registry%/}
releases=${MOCK_RELEASES_URL:-http://127.0.0.1:${MOCK_AGENT_RELEASES_PORT:-8082}}
releases=${releases%/}
timeout=${TIMEOUT:-60}
agent_token=${AGENT_TOKEN:-dev-registry-agent-token}

root=$(cd "$(dirname "$0")/.." && pwd)
agents_file=$root/dev/agents.yaml
# The card URLs the registry lists, as the orchestrator reaches them inside the compose network.
card_releases=http://mock-agent-releases:8080/.well-known/agent-card.json
card_agent=http://mock-agent:8080/.well-known/agent-card.json

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "registry e2e passed"; else echo "registry e2e FAILED"; exit 1; fi
}
# check DESCRIPTION ACTUAL EXPECTED
check() {
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx
  curl --fail-with-body -sS --max-time 30 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done
[ -f "$agents_file" ] || { echo "FAIL $agents_file does not exist" >&2; exit 1; }

# The ids of dev/agents.yaml, in order: what the orchestrator lists first, whatever the registry says.
static_ids=$(sed -n 's/^- id: *//p' "$agents_file" | paste -sd' ' -)
[ -n "$static_ids" ] || { echo "FAIL $agents_file lists no agent" >&2; exit 1; }

# ids SOURCE: the ids GET /api/agents lists, in order, as one line; SOURCE `all` lists every agent.
ids() {
  api GET /api/agents 2>/dev/null | jq -r --arg source "$1" \
    '[.[] | select($source == "all" or .source == $source) | .id] | join(" ")' 2>/dev/null || true
}

# lists ID: succeeds when GET /api/agents lists the agent ID.
lists() {
  case " $(ids all) " in *" $1 "*) return 0 ;; *) return 1 ;; esac
}

# lists_none_of_the_registry: succeeds when GET /api/agents lists no agent with `source: registry`.
lists_none_of_the_registry() {
  [ -z "$(ids registry)" ]
}

# wait_for DESCRIPTION COMMAND...: runs COMMAND once a second for 10 s, ok when it succeeds. The registry
# says `Cache-Control: max-age=2`, so a change shows within a few seconds, and never needs a restart.
wait_for() {
  _what=$1
  shift
  _deadline=$(( $(date +%s) + 10 ))
  while :; do
    if "$@" >/dev/null 2>&1; then ok "$_what"; return 0; fi
    if [ "$(date +%s)" -ge "$_deadline" ]; then bad "$_what (not within 10 s)"; return 1; fi
    sleep 1
  done
}

# stub NAME BODY: adds a stub to mock-registry that wins over the file stubs (priority 1, and the latest of the
# stubs with the same priority wins). The stubs of the file are `priority 1` (304) and `priority 2` (200).
stub() {
  curl -fsS --max-time 10 -X POST "$registry/__admin/mappings" -H 'content-type: application/json' \
    -d "$(jq -n --arg name "$1" --argjson response "$2" \
      '{name: $name, priority: 1, request: {method: "GET", urlPath: "/registry/v1/agents"}, response: $response}')" >/dev/null
}

# listing ITEMS...: the response of a stub that lists these items (JSON objects) as agent-registry/v1.
listing() {
  jq -n --argjson items "$1" '{
    status: 200,
    headers: {"Content-Type": "application/linkset+json", "Cache-Control": "private, max-age=2"},
    jsonBody: {linkset: [{profile: [{href: "https://agents.vymalo.com/registry/v1"}], item: $items}]}}'
}

# --- is the stack there, and does it read a registry? -----------------------------------------------------------
if ! curl -fsS --max-time 10 "$base/readyz" >/dev/null 2>&1; then
  echo "Nothing answers at $base/readyz. Start the stack first: docker compose --profile app up -d --build --wait" >&2
  exit 2
fi
if ! curl -fsS --max-time 10 "$registry/__admin/health" >/dev/null 2>&1; then
  echo "Nothing answers at $registry/__admin/health: is mock-registry up (docker compose up -d --wait)?" >&2
  exit 2
fi
if ! api GET /api/registry >/dev/null 2>&1; then
  echo "GET /api/registry failed: is this an orchestrator that reads a registry (AGENT_REGISTRY_URL in compose.yaml)?" >&2
  exit 2
fi

# A clean start: the file stubs only, and an empty request journal for the agent behind `platform-coder`.
curl -fsS --max-time 10 -X POST "$registry/__admin/mappings/reset" >/dev/null
curl -fsS --max-time 10 -X DELETE "$releases/__admin/requests" >/dev/null

# --- (a) the registry's agent is listed, with the releases of its own card -------------------------------------
echo "== (a) the registry's agent"
wait_for "GET /api/agents lists platform-coder (the registry's agent)" lists platform-coder
agents=$(api GET /api/agents 2>/dev/null || echo '[]')
check "the agents of dev/agents.yaml come first, in order, then platform-coder" \
  "$(printf '%s' "$agents" | jq -r '[.[].id] | join(" ")')" "$static_ids platform-coder"
check "every agent of dev/agents.yaml is source static" "$(ids static)" "$static_ids"
check "and none of them has tags (the registry's labels, which a static agent has none of)" \
  "$(printf '%s' "$agents" | jq -r '[.[] | select(.source == "static") | (.tags // []) | length] | add // 0')" "0"
check "platform-coder is source registry, titled and tagged by the registry" \
  "$(printf '%s' "$agents" | jq -r '.[] | select(.id == "platform-coder") | [.source, .name, (.tags | join("+"))] | join("|")')" \
  "registry|Platform coder|coding+git"
check "its card URL is the registry's item" \
  "$(printf '%s' "$agents" | jq -r '.[] | select(.id == "platform-coder") | .cardUrl')" "$card_releases"
check "its releases are those of its own card, not the registry's" \
  "$(printf '%s' "$agents" | jq -r '.[] | select(.id == "platform-coder") | [.releases.defaultChannel, (.releases.channels | keys | join("+"))] | join("|")')" \
  "production|latest+production+staging"
check "GET /api/registry: the static list and the platform's registry are both ok" \
  "$(api GET /api/registry 2>/dev/null | jq -c '[.sources[] | "\(.name):\(.status)"]' 2>/dev/null || true)" '["static:ok","platform:ok"]'

thread=$(uuid)
input=$(jq -n --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: "hi from the registry"}], forwardedProps: {}}')
echo "thread $thread (platform-coder): hi from the registry"
code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/platform-coder" \
  -H "$id_header" -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" 2>"$tmp/err" || true)
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/platform-coder answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
state=
_deadline=$(( $(date +%s) + timeout ))
while :; do
  state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
  case $state in done | blocked | failed | cancelled) break ;; esac
  if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
  sleep 1
done
check "a thread on platform-coder is delegated and ends done" "$state" "done"
sent=$(curl -fsS --max-time 10 "$releases/__admin/requests" 2>/dev/null |
  jq -r '[.requests[] | select(.request.url == "/a2a" and .request.method == "POST") | .request.headers.Authorization // .request.headers.authorization // ""] | unique | join(",")' 2>/dev/null || true)
check "the mock agent saw the bearer token of AGENT_REGISTRY_AGENT_TOKEN" "$sent" "Bearer $agent_token"

# --- (b) an agent added to the registry shows up, without a restart ---------------------------------------------
echo "== (b) an agent added to the registry"
items=$(jq -n --arg releases "$card_releases" --arg agent "$card_agent" '[
  {href: $releases, type: "application/json", title: "Platform coder", service: ["platform-coder"], tags: ["coding", "git"]},
  {href: $agent, type: "application/json", title: "Platform helper", service: ["platform-helper"], tags: ["writing"]}]')
stub "registry lists platform-helper too" "$(listing "$items")"
wait_for "GET /api/agents lists platform-helper within 10 s, with no restart" lists platform-helper
check "the registry's agents follow the static ones, in the registry's order" \
  "$(ids all)" "$static_ids platform-coder platform-helper"
check "platform-helper is source registry and tagged writing" \
  "$(api GET /api/agents 2>/dev/null | jq -r '.[] | select(.id == "platform-helper") | [.source, (.tags | join("+"))] | join("|")' 2>/dev/null || true)" \
  "registry|writing"

# --- (c) a registry that is down leaves the static agents and says so -------------------------------------------
echo "== (c) the registry is down"
stub "registry is down" '{"status": 503}'
wait_for "GET /api/agents lists none of the registry's agents within 10 s" lists_none_of_the_registry
check "the static agents of dev/agents.yaml stay, in order" "$(ids all)" "$static_ids"
check "GET /api/registry says the platform's registry is unavailable, and why, with no URL" \
  "$(api GET /api/registry 2>/dev/null | jq -r '[.sources[] | select(.name == "platform") | .status, .detail] | join("|")' 2>/dev/null || true)" \
  "unavailable|the registry could not be reached"
check "GET /api/registry still says the static list is ok" \
  "$(api GET /api/registry 2>/dev/null | jq -r '[.sources[] | select(.name == "static") | .status] | join(",")' 2>/dev/null || true)" "ok"
input=$(jq -n --arg thread "$(uuid)" --arg run "$(uuid)" --arg msg "$(uuid)" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: "hi"}], forwardedProps: {}}')
curl -sS --max-time 30 -D "$tmp/headers" -o "$tmp/refused.json" -w '%{http_code}' -X POST "$base/agui/agents/platform-coder" \
  -H "$id_header" -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" >"$tmp/status" 2>/dev/null || true
check "a run on platform-coder is a 503, not a 404: the registry cannot say" "$(cat "$tmp/status")" "503"
if grep -qi '^retry-after:' "$tmp/headers"; then ok "the 503 says when to try again (Retry-After)"; else bad "the 503 has no Retry-After"; fi
check "the problem says the registry is unreachable" "$(jq -r '.detail // empty' "$tmp/refused.json" 2>/dev/null || true)" "the agent registry is unreachable"
# the first static agent that has no gate and answers at once: the mock coder
thread=$(uuid)
input=$(jq -n --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: "hi while the registry is down"}], forwardedProps: {}}')
code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run2.sse" -w '%{http_code}' -X POST "$base/agui/agents/mock-coder" \
  -H "$id_header" -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" 2>"$tmp/err" || true)
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/mock-coder answered HTTP ${code:-none} while the registry was down"
else
  state=
  _deadline=$(( $(date +%s) + timeout ))
  while :; do
    state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
    case $state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 1
  done
  check "a thread on a static agent still ends done while the registry is down" "$state" "done"
fi

# --- (d) the registry is read again when it answers --------------------------------------------------------------
echo "== (d) the registry answers again"
curl -fsS --max-time 10 -X POST "$registry/__admin/mappings/reset" >/dev/null
wait_for "GET /api/agents lists platform-coder again within 10 s" lists platform-coder
check "platform-helper is gone with the stub that listed it" "$(ids all)" "$static_ids platform-coder"
check "GET /api/registry says both sources are ok again" \
  "$(api GET /api/registry 2>/dev/null | jq -c '[.sources[] | "\(.name):\(.status)"]' 2>/dev/null || true)" '["static:ok","platform:ok"]'

finish
