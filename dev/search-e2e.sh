#!/usr/bin/env sh
# System-level test of searching a person's own threads (ADR 0068, docs/api/thread-search-v1.md): `POST /api/search/threads` finds
# threads by the words of their titles, descriptions and messages, and only the caller's own.
#
#   dev/search-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; this scenario does not use it, but the profile
# starts it):
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-coder` of dev/agents.yaml: the WireMock A2A mock (dev/wiremock/agent), which answers every message with a
# pull request and says "Done. The pull request is ready for review." when it completes. The words of the threads are made up for
# the run (a letters-only run id in front of each, so that nothing of an earlier run or of another scenario can be found by them).
# The user is `search@example.com` of dev/mock-oidc/users.json, a person no other scenario signs in as. The script speaks what the
# web speaks (docs/api/chat-api.yaml): `searchThreads`, `patchThread` (a rename), `arrangeThread` (archive), `forkThread`,
# `deleteThread` and `listThreads`.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * three threads of one person, each with a word of its own and one word in common; the answer is validated for its shape
#     (terms, results, rank, hits with `seq`, `role` and a snippet whose marked words are the words searched for) and is never cached;
#   * WORDS: a person's message is found by a word of it, by a prefix, in capitals and with its accents folded (`cafe` finds
#     `Café`, `coeur` finds `cœur`); every term must be in one document (two words of one message find it, one word of each of two
#     messages does not); the agent's FINAL answer is found, with `role: agent`;
#   * RANK: a rename that puts the common word in a title ranks that thread first, then the others, newest first;
#   * ARCHIVED: archived threads are in the results by default, left out by `archived: exclude` and alone with `only`;
#   * A FORK: the messages it copied from its parent are not its hits while the parent exists; its own message is;
#   * OTHERS: another person (the administrator) finds none of these threads, by a word of the title or of a message; nothing is
#     found through `/api/shared` or `/api/public`;
#   * HOSTILE: text that looks like SQL, a `tsquery` or a quote is words (the terms say so) and breaks nothing: the thread is still
#     found afterwards;
#   * REFUSALS: a blank query is 400 `query_empty`, 201 characters 400 `query_too_long`, a cursor that is not this request's 400
#     `bad_cursor`, an `owner` member 400 `bad_request`, a body that is not JSON 415; a query that keeps no term is 200 with `terms: []`;
#   * DELETE: a deleted thread is not found by its title or its words, at once;
#   * THE LOG: no word of any query is in a line of the orchestrator's log (read with `docker compose logs` when docker is there:
#     otherwise this one check is skipped and says so).
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      search@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   OTHER_EMAIL     admin@example.com, another user, who must find none of the first one's threads
#   AGENT_ID        mock-coder, the agent the threads talk to
#   ORCH_SERVICE    orchestrator, the compose service whose log is read
#   TIMEOUT         90    seconds to wait for a thread to stop
#
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-search@example.com}
other=${OTHER_EMAIL:-admin@example.com}
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
other_header=$(sh "$(dirname "$0")/auth-header.sh" "$other")
agent=${AGENT_ID:-mock-coder}
service=${ORCH_SERVICE:-orchestrator}
timeout=${TIMEOUT:-90}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

tmp=$(mktemp -d)
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

finish() {
  if [ "$fail" -eq 0 ]; then echo "search e2e passed"; else echo "search e2e FAILED"; exit 1; fi
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }
# The clock is read once: a clock that steps back between two readings would put a later id before an earlier one.
now_ms=$(date +%s%3N 2>/dev/null)
case $now_ms in *[!0-9]* | '') now_ms=$(($(date +%s) * 1000)) ;; esac
uuid7() { # uuid7 N: a time-ordered id (UUIDv7), as the web makes a thread's; N orders the ids of this script
  hex=$(printf '%012x' "$((now_ms + $1))")
  r=$(uuid | tr -d -)
  printf '%s-%s-7%s-%s-%s\n' "$(printf %s "$hex" | cut -c1-8)" "$(printf %s "$hex" | cut -c9-12)" \
    "$(printf %s "$r" | cut -c14-16)" "$(printf %s "$r" | cut -c17-20)" "$(printf %s "$r" | cut -c21-32)"
}

expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

api() { # api METHOD PATH [BODY]: the body on stdout, non-zero when the status is not 2xx (the resource API)
  if [ $# -ge 3 ]; then
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header" \
      -H 'content-type: application/json' -d "$3"
  else
    curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
  fi
}

call() { # call HEADER METHOD PATH [BODY]: the HTTP status, the body in $tmp/body
  _h=$1 _m=$2 _p=$3
  if [ $# -ge 4 ]; then
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$_m" "$base$_p" -H "$_h" \
      -H 'content-type: application/json' -d "$4"
  else
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$_m" "$base$_p" -H "$_h"
  fi
}

find_as() { # find_as HEADER BODY: the HTTP status of a search, the page in $tmp/body
  call "$1" POST /api/search/threads "$2"
}

find() { # find BODY: the page of the person's own search on stdout (the status must be 200)
  api POST /api/search/threads "$1"
}

ids_of() { # ids_of PAGE: the ids of the results among THIS script's threads, in order, comma-separated
  printf '%s' "$1" | jq -r --argjson ours "$mine" '[.results[].thread.id | select(. as $i | $ours | index($i))] | join(",")'
}

say() { # say THREAD TEXT: one run (a message), to its end; prints the HTTP status
  _input=$(jq -n --arg thread "$1" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/$agent" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

wait_state() { # wait_state THREAD STATE: the resource API's view says STATE (within TIMEOUT seconds)
  _n=0
  while [ "$(api GET "/api/threads/$1" | jq -r .state 2>/dev/null)" != "$2" ]; do
    _n=$((_n + 1))
    [ "$_n" -lt $((timeout * 5)) ] || return 1
    sleep 0.2
  done
}

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

case " $(api GET /api/agents | jq -r '[.[].id] | join(" ")') " in
  *" $agent "*) ;;
  *) echo "the agent '$agent' is not listed by GET /api/agents: is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac

# A run id made of letters only (a digit would be a word of its own): every word below starts with it, so nothing else matches.
run=$(uuid | tr -d - | cut -c1-12 | tr '0-9' 'g-p')
common="${run}common"
t1=$(uuid7 1)
t2=$(uuid7 2)
t3=$(uuid7 3)
mine=$(jq -n --arg a "$t1" --arg b "$t2" --arg c "$t3" '[$a, $b, $c]')

echo "== is the route there"
cfg=$(api GET /api/config)
expect "GET /api/config says search is served, and what it bounds" \
  "$(printf '%s' "$cfg" | jq -c '.search')" '{"maxQueryLength":200,"maxTerms":8}'

echo "== three threads, each with a word of its own and one in common"
expect "thread one is accepted" "$(say "$t1" "Search e2e ${run}one $common Café cœur and the Straße")" "200"
expect "thread two is accepted" "$(say "$t2" "Search e2e ${run}two $common orchard")" "200"
expect "thread three is accepted" "$(say "$t3" "Search e2e ${run}three $common harbour and ${run}tide")" "200"
for t in "$t1" "$t2" "$t3"; do
  if wait_state "$t" "done"; then ok "$t ends done"; else bad "$t never reached done"; fi
done

echo "== words: found by a word, a prefix, capitals, accents folded"
page=$(find "$(jq -n --arg q "${run}one" '{q: $q}')" || true)
expect "the word of thread one finds it, and only it" "$(ids_of "$page")" "$t1"
expect "the terms say what was searched for" "$(printf '%s' "$page" | jq -c '.query.terms')" "[\"${run}one\"]"
hit=$(printf '%s' "$page" | jq -c --arg t "$t1" '.results[] | select(.thread.id == $t)')
expect "the person's message is a hit with its seq and role" \
  "$(printf '%s' "$hit" | jq -r '[.hits[] | select(.field == "message")] | .[0] | [(.seq | type), .role] | join(" ")')" "number person"
expect "the marked word of the snippet is the word searched for" \
  "$(printf '%s' "$hit" | jq -r '[.hits[] | select(.field == "message")] | .[0].snippet | .matches[0] as $m | .text[$m.start:$m.end]')" "${run}one"
expect "the rank is 0: only a message matches" "$(printf '%s' "$hit" | jq -r .rank)" "0"
expect "messageHits says how many messages match" "$(printf '%s' "$hit" | jq -r .messageHits)" "1"
expect "a prefix finds it" "$(ids_of "$(find "$(jq -n --arg q "$(printf %s "${run}on")" '{q: $q}')")")" "$t1"
expect "capitals find it" "$(ids_of "$(find "$(jq -n --arg q "$(printf %s "${run}ONE" | tr 'a-z' 'A-Z')" '{q: $q}')")")" "$t1"
expect "cafe finds Café" "$(ids_of "$(find "$(jq -n --arg q "${run}one cafe" '{q: $q}')")")" "$t1"
expect "COEUR finds cœur" "$(ids_of "$(find "$(jq -n --arg q "${run}one COEUR" '{q: $q}')")")" "$t1"
expect "strasse finds Straße" "$(ids_of "$(find "$(jq -n --arg q "${run}one strasse" '{q: $q}')")")" "$t1"
expect "two words of one message find it" "$(ids_of "$(find "$(jq -n --arg q "${run}three ${run}tide" '{q: $q}')")")" "$t3"
expect "one word of each of two messages does not" "$(ids_of "$(find "$(jq -n --arg q "${run}one ${run}two" '{q: $q}')")")" ""
expect "the middle of a word is not a prefix" "$(ids_of "$(find "$(jq -n --arg q "ne ${common}" '{q: $q}')")")" ""

echo "== the agent's final answer"
export_json=$(api GET "/api/threads/$t1/export" || true)
expect "thread one holds a final message of the agent" \
  "$(printf '%s' "$export_json" | jq -r '[.events[] | select(.kind == "agent_message" and .data.final == true)] | length > 0')" "true"
page=$(find "$(jq -n '{q: "ready review"}')" || true)
agent_hits=$(printf '%s' "$page" | jq -r --arg t "$t1" '[.results[] | select(.thread.id == $t) | .hits[] | select(.field == "message" and .role == "agent")] | length')
expect "its words are found, as the agent's" "$([ "$agent_hits" -ge 1 ] && echo yes || echo no)" "yes"

echo "== rank: a title beats a message, then the newest thread"
expect "the common word is in all three messages, newest first" "$(ids_of "$(find "$(jq -n --arg q "$common" '{q: $q}')")")" "$t3,$t2,$t1"
expect "renaming is 200" "$(call "$id_header" PATCH "/api/threads/$t1" "$(jq -n --arg t "Ranked $common" '{title: $t}')")" "200"
page=$(find "$(jq -n --arg q "$common" '{q: $q}')" || true)
expect "the thread whose title has the word is first" "$(ids_of "$page")" "$t1,$t3,$t2"
expect "its rank is 2 and its first hit is the title" \
  "$(printf '%s' "$page" | jq -r --arg t "$t1" '.results[] | select(.thread.id == $t) | [.rank, .hits[0].field] | join(" ")')" "2 title"

echo "== archived: in by default, out when asked"
expect "archiving is 200" "$(call "$id_header" PATCH "/api/threads/$t2/rail" '{"archived":true}')" "200"
expect "an archived thread is found by default" "$(ids_of "$(find "$(jq -n --arg q "$common" '{q: $q}')")")" "$t1,$t3,$t2"
expect "and says it is archived" \
  "$(find "$(jq -n --arg q "$common" '{q: $q}')" | jq -r --arg t "$t2" '.results[] | select(.thread.id == $t) | .thread.archived')" "true"
expect "archived: exclude leaves it out" "$(ids_of "$(find "$(jq -n --arg q "$common" '{q: $q, archived: "exclude"}')")")" "$t1,$t3"
expect "archived: only lists it alone" "$(ids_of "$(find "$(jq -n --arg q "$common" '{q: $q, archived: "only"}')")")" "$t2"
expect "the results are among what listThreads lists" \
  "$(api GET '/api/threads?limit=100&archived=include' | jq -r --argjson ours "$mine" '[.[].id | select(. as $i | $ours | index($i))] | length')" "3"

echo "== a fork: its copied messages are its parent's"
fork=$(uuid7 4)
mine=$(jq -n --arg a "$t1" --arg b "$t2" --arg c "$t3" --arg d "$fork" '[$a, $b, $c, $d]')
expect "the fork is created" "$(call "$id_header" POST "/api/threads/$t1/fork" "$(jq -n --arg id "$fork" '{after: 1, id: $id}')")" "201"
expect "the words of the parent's message are the parent's, not the fork's" \
  "$(ids_of "$(find "$(jq -n --arg q "${run}one cafe" '{q: $q}')")")" "$t1"
expect "the fork says another message" "$(say "$fork" "Search e2e ${run}forked words")" "200"
if wait_state "$fork" "done"; then ok "the fork ends done"; else bad "the fork never reached done"; fi
page=$(find "$(jq -n --arg q "${run}forked" '{q: $q}')" || true)
expect "its own message finds it" "$(ids_of "$page")" "$fork"
expect "the hit is a message after the cut" \
  "$(printf '%s' "$page" | jq -r --arg t "$fork" '.results[] | select(.thread.id == $t) | .hits[0] | [.field, .role] | join(" ")')" "message person"
fork_cut=$(api GET "/api/threads/$fork" | jq -r '.forkedFrom.seq')
expect "its seq is after the events it copied" \
  "$(printf '%s' "$page" | jq -r --arg t "$fork" --argjson cut "$fork_cut" '.results[] | select(.thread.id == $t) | (.hits[0].seq > $cut)')" "true"

echo "== another person finds none of it"
expect "the administrator's search of the common word is 200" "$(find_as "$other_header" "$(jq -n --arg q "$common" '{q: $q}')")" "200"
expect "and finds nothing" "$(jq -r '.results | length' "$tmp/body")" "0"
expect "nor by a word of a title" "$(find_as "$other_header" "$(jq -n '{q: "Ranked"}')")" "200"
expect "none of ours is in what they find" \
  "$(jq -r --argjson ours "$mine" '[.results[].thread.id | select(. as $i | $ours | index($i))] | length' "$tmp/body")" "0"
for path in /api/shared/search /api/shared/search/threads /api/public/search/threads /api/public/shared/search; do
  code=$(call "$id_header" POST "$path" "$(jq -n --arg q "$common" '{q: $q}')")
  case $code in
    404 | 405) ok "$path is no way to search ($code)" ;;
    *) bad "$path answered $code: nothing is searched through a link" ;;
  esac
done

echo "== text that looks like syntax is words"
expect "SQL is words: terms say so" "$(find_as "$id_header" "$(jq -n '{q: "'"'"' OR 1=1 --"}')") $(jq -c .query.terms "$tmp/body")" '200 ["or"]'
expect "a tsquery is words" "$(find_as "$id_header" "$(jq -n '{q: "a:* | b & !c <-> dd"}')") $(jq -c .query.terms "$tmp/body")" '200 ["dd"]'
expect "quotes and a semicolon are words" "$(find_as "$id_header" "$(jq -n '{q: "\"; DROP TABLE threads; --"}')") $(jq -c .query.terms "$tmp/body")" '200 ["drop","table","threads"]'
expect "the threads are still found" "$(ids_of "$(find "$(jq -n --arg q "${run}one" '{q: $q}')")")" "$t1"
expect "a query that keeps no term is 200 with no term" "$(find_as "$id_header" '{"q":"!!"}') $(jq -c '[.query.terms, .results]' "$tmp/body")" '200 [[],[]]'

echo "== refusals carry the codes of the contract"
expect "a blank query is 400" "$(find_as "$id_header" '{"q":"  "}')" "400"
expect "it is query_empty" "$(jq -r .code "$tmp/body")" "query_empty"
long=$(printf '%0201d' 0 | tr 0 x)
expect "201 characters are 400" "$(find_as "$id_header" "$(jq -n --arg q "$long" '{q: $q}')")" "400"
expect "it is query_too_long" "$(jq -r .code "$tmp/body")" "query_too_long"
expect "an owner member is 400" "$(find_as "$id_header" "$(jq -n --arg o "$email" '{q: "x1", owner: $o}')")" "400"
expect "it is bad_request" "$(jq -r .code "$tmp/body")" "bad_request"
expect "a limit out of range is 400" "$(find_as "$id_header" '{"q":"xx","limit":51}')" "400"
expect "a cursor that is nobody's is 400" "$(find_as "$id_header" '{"q":"xx","cursor":"nonsense"}')" "400"
expect "it is bad_cursor" "$(jq -r .code "$tmp/body")" "bad_cursor"
expect "a body that is not JSON is 415" \
  "$(curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X POST "$base/api/search/threads" -H "$id_header" -H 'content-type: text/plain' -d 'q=xx')" "415"
expect "without a sign-in it is 401" \
  "$(curl -sS --max-time 60 -o /dev/null -w '%{http_code}' -X POST "$base/api/search/threads" -H 'content-type: application/json' -d '{"q":"xx"}')" "401"

echo "== pages: a cursor is for its own request"
page=$(find "$(jq -n --arg q "$common" '{q: $q, limit: 1}')" || true)
expect "a page of one has the newest first and a next" \
  "$(printf '%s' "$page" | jq -r '[(.results | length), (.next | type)] | join(" ")')" "1 string"
next=$(printf '%s' "$page" | jq -r .next)
expect "the next page goes on from there" \
  "$(find "$(jq -n --arg q "$common" --arg c "$next" '{q: $q, limit: 1, cursor: $c}')" | jq -r '.results | length')" "1"
expect "another query's cursor is 400" "$(find_as "$id_header" "$(jq -n --arg q "${common}x" --arg c "$next" '{q: $q, limit: 1, cursor: $c}')")" "400"
expect "so is one with another filter" "$(find_as "$id_header" "$(jq -n --arg q "$common" --arg c "$next" '{q: $q, limit: 1, archived: "only", cursor: $c}')")" "400"

echo "== a deleted thread is gone at once"
expect "thread three is found by its words" "$(ids_of "$(find "$(jq -n --arg q "${run}tide" '{q: $q}')")")" "$t3"
expect "deleting it is 204" "$(call "$id_header" DELETE "/api/threads/$t3")" "204"
expect "its words find nothing" "$(ids_of "$(find "$(jq -n --arg q "${run}tide" '{q: $q}')")")" ""
# the fork has its parent's title, which holds the common word: rank 2, and the newest
expect "nor does the word it shared" "$(ids_of "$(find "$(jq -n --arg q "$common" '{q: $q}')")")" "$fork,$t1,$t2"

echo "== the log"
if command -v docker >/dev/null 2>&1 && docker compose ps -q "$service" >/dev/null 2>&1 && [ -n "$(docker compose ps -q "$service" 2>/dev/null)" ]; then
  docker compose logs --no-color "$service" >"$tmp/log" 2>&1 || true
  if [ -s "$tmp/log" ]; then
    expect "the log has the requests" "$(grep -c '/api/search/threads' "$tmp/log" | sed 's/^0$/none/;s/^[1-9][0-9]*$/some/')" "some"
    for word in "$run" "$common" "DROP TABLE" "1=1"; do
      if grep -q -F -- "$word" "$tmp/log"; then bad "'$word' is in the orchestrator's log"; else ok "'$word' is in no line of the log"; fi
    done
  else
    echo "skip the log is empty: the service '$service' is not the one of this stack"
  fi
else
  echo "skip no docker compose service '$service' here: the log is not read"
fi

finish
