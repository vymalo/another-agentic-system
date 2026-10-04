#!/usr/bin/env sh
# System-level test of the person's own list of threads (ADR 0042): pin, order, archive and eject over the API. What a
# person does to their list is a change of the thread's row and no event of its log: nothing of it reaches a fork, the
# export's events or the people a thread is shared with, and it needs ownership and `thread.read`, not `thread.write`.
#
#   dev/rail-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; this scenario does not use
# it, but the profile starts it):
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-coder` of dev/agents.yaml: the WireMock A2A mock (dev/wiremock/agent), which answers every
# message with a pull request and `completed`. What it says does not matter here: the threads are only there to be
# arranged. The script speaks what the web speaks (docs/api/chat-api.yaml): `listThreads` with `order=rail` and
# `archived`, `arrangeThread` (`PATCH /api/threads/{id}/rail`), `forkThread` for a thread to nest, and the thread, its
# export and the shared view.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * three threads of one person: a new thread is on top of the list in the person's own order (`order=rail`), the
#     default order (`recent`) is the one the list always had;
#   * PIN: `{pinned: true}` is 200 and the thread says `pinned: true`; the pinned come first in the list in the
#     person's order, the default order does not move, `GET /api/threads/{id}` and the export's thread say it; the same
#     request again is 200 and the same thread;
#   * ORDER: `{place: "top"}`, `{place: {before: id}}` and `{place: {after: id}}` put a thread on top, before and after
#     another, and the others keep their order; an anchor that is not a thread of the person's is 422 `bad_anchor`; an
#     unknown member, no member and a `place` that is not one are 400;
#   * ARCHIVE: `{archived: true}` leaves the thread out of the list by default and lists it with `archived=only` and
#     `archived=include`; `{archived: false}` brings it back where it was;
#   * EJECT: a fork made from a thread (`forkThread`, `{after: 1}`) is nested under its parent (`nestedUnder`), its
#     parent's block holds it in the person's order, it is neither pinned nor placed (422 `nested_row`); `{nested: false}`
#     ejects it: `nestedUnder` goes, `forkedFrom` stays, and moving the parent no longer carries it; `{nested: true}`
#     is 422;
#   * none of it is in the log: the events of a thread, its `lastSeq` and its `updatedAt` are what they were before it
#     was arranged, and another person's request (the administrator's too) is a 404 that changes nothing;
#   * it leaves nothing pinned.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   OTHER_EMAIL     admin@example.com, another user, who must not be able to arrange the first one's threads
#   AGENT_ID        mock-coder, the agent the threads talk to
#   TIMEOUT         90    seconds to wait for a thread to stop
#
# The list may hold other threads (the other scenarios' threads of the same user): every check looks at the order of
# THIS script's threads among themselves. Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
other=${OTHER_EMAIL:-admin@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
other_header=$(sh "$(dirname "$0")/auth-header.sh" "$other")
agent=${AGENT_ID:-mock-coder}
timeout=${TIMEOUT:-90}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

tmp=$(mktemp -d)
pinned_ids=""
cleanup() {
  # leave nothing pinned: the next run, and the other scenarios, share this user's list
  for _t in $pinned_ids; do
    curl -sS --max-time 30 -o /dev/null -X PATCH "$base/api/threads/$_t/rail" -H "$id_header" \
      -H 'content-type: application/json' -d '{"pinned":false}' 2>/dev/null || true
  done
  rm -rf "$tmp"
}
trap cleanup EXIT

finish() {
  if [ "$fail" -eq 0 ]; then echo "rail e2e passed"; else echo "rail e2e FAILED"; exit 1; fi
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

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

arrange() { # arrange THREAD BODY: the HTTP status of the person's request, the thread in $tmp/body
  call "$id_header" PATCH "/api/threads/$1/rail" "$2"
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

# the ids of THIS script's threads, in the order a listing gives them: the whole list is read (a hundred at most) and
# filtered, since other scenarios' threads may be in it
mine="" # set below: a jq array of the three ids
order_of() { # order_of QUERY: the ids among ours, in the order of the listing QUERY, comma-separated
  api GET "/api/threads?limit=100$1" | jq -r --argjson ours "$mine" \
    '[.[].id | select(. as $i | $ours | index($i))] | join(",")'
}

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

case " $(api GET /api/agents | jq -r '[.[].id] | join(" ")') " in
  *" $agent "*) ;;
  *) echo "the agent '$agent' is not listed by GET /api/agents: is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac

t1=$(uuid)
t2=$(uuid)
t3=$(uuid)
mine=$(jq -n --arg a "$t1" --arg b "$t2" --arg c "$t3" '[$a, $b, $c]')

echo "== three threads: a new one is on top"
for t in "$t1" "$t2" "$t3"; do
  expect "thread $t is accepted" "$(say "$t" "Rail e2e $(uuid | cut -c1-8)")" "200"
  if wait_state "$t" "done"; then ok "it ends done"; else bad "it never reached done"; fi
done
expect "the list in the person's own order has the newest on top" "$(order_of '&order=rail')" "$t3,$t2,$t1"
expect "the default order is the one the list always had" "$(order_of '')" "$t3,$t2,$t1"
api GET "/api/threads/$t1/export" >"$tmp/export-before.json" || true
last_seq=$(api GET "/api/threads/$t1" | jq -r .lastSeq)
updated_at=$(api GET "/api/threads/$t1" | jq -r .updatedAt)
events_before=$(jq -c '[.events[] | [.seq, .kind]]' "$tmp/export-before.json")

echo "== pin: on top of the pinned, and the pinned come first"
pinned_ids="$t1"
expect "pinning is 200" "$(arrange "$t1" '{"pinned":true}')" "200"
expect "the thread says it is pinned" "$(jq -r '[.id == $id, .pinned] | join(" ")' --arg id "$t1" "$tmp/body")" "true true"
cp "$tmp/body" "$tmp/pinned.json"
expect "the pinned thread is first, the others keep their order" "$(order_of '&order=rail')" "$t1,$t3,$t2"
expect "the default order does not move" "$(order_of '')" "$t3,$t2,$t1"
expect "the list says which thread is pinned" \
  "$(api GET '/api/threads?limit=100' | jq -r --arg id "$t1" '.[] | select(.id == $id) | .pinned')" "true"
expect "GET /api/threads/{id} says it" "$(api GET "/api/threads/$t1" | jq -r .pinned)" "true"
expect "the same request again is 200" "$(arrange "$t1" '{"pinned":true}')" "200"
expect "and the same thread, nothing written" "$(jq -S . "$tmp/body")" "$(jq -S . "$tmp/pinned.json")"

echo "== order: on top, before and after another"
expect "{place: top} is 200" "$(arrange "$t2" '{"place":"top"}')" "200"
expect "t2 is on top of the rest, under the pinned" "$(order_of '&order=rail')" "$t1,$t2,$t3"
expect "{place: {before}} is 200" "$(arrange "$t3" "$(jq -n --arg id "$t2" '{place: {before: $id}}')")" "200"
expect "t3 is before t2" "$(order_of '&order=rail')" "$t1,$t3,$t2"
expect "{place: {after}} is 200" "$(arrange "$t3" "$(jq -n --arg id "$t2" '{place: {after: $id}}')")" "200"
expect "t3 is after t2" "$(order_of '&order=rail')" "$t1,$t2,$t3"
expect "an anchor that is nobody's is 422" "$(arrange "$t2" "$(jq -n --arg id "$(uuid)" '{place: {before: $id}}')")" "422"
expect "it is bad_anchor" "$(jq -r .code "$tmp/body")" "bad_anchor"
expect "a thread is no anchor for itself (422)" "$(arrange "$t2" "$(jq -n --arg id "$t2" '{place: {after: $id}}')")" "422"
expect "an unknown member is 400" "$(arrange "$t2" '{"colour":"red"}')" "400"
expect "no member is 400" "$(arrange "$t2" '{}')" "400"
expect "a place that is not one is 400" "$(arrange "$t2" '{"place":"middle"}')" "400"
expect "the order is what it was" "$(order_of '&order=rail')" "$t1,$t2,$t3"

echo "== archive: out of the list by default, back where it was"
expect "archiving is 200" "$(arrange "$t3" '{"archived":true}')" "200"
expect "the thread says it is archived" "$(jq -r .archived "$tmp/body")" "true"
expect "it is left out of the list" "$(order_of '&order=rail')" "$t1,$t2"
expect "and of the default order" "$(order_of '')" "$t2,$t1"
expect "archived=only lists it" "$(order_of '&order=rail&archived=only')" "$t3"
expect "archived=include lists all three, the archived last" "$(order_of '&order=rail&archived=include')" "$t1,$t2,$t3"
expect "unarchiving is 200" "$(arrange "$t3" '{"archived":false}')" "200"
expect "it says no more" "$(jq -r 'has("archived")' "$tmp/body")" "false"
expect "it is back where it was" "$(order_of '&order=rail')" "$t1,$t2,$t3"

echo "== eject: a fork is nested under its parent, and leaves it when the person says"
fork=$(uuid)
code=$(call "$id_header" POST "/api/threads/$t2/fork" "$(jq -n --arg id "$fork" '{after: 1, id: $id}')")
expect "the fork is created" "$code" "201"
expect "it is nested under its parent and says where it was forked from" \
  "$(jq -r '[.nestedUnder, .forkedFrom.threadId] | join(" ")' "$tmp/body")" "$t2 $t2"
mine=$(jq -n --arg a "$t1" --arg b "$t2" --arg c "$t3" --arg d "$fork" '[$a, $b, $c, $d]')
expect "the block of t2 holds the fork, right after it" "$(order_of '&order=rail')" "$t1,$t2,$fork,$t3"
expect "a nested thread is not pinned (422)" "$(arrange "$fork" '{"pinned":true}')" "422"
expect "it is nested_row" "$(jq -r .code "$tmp/body")" "nested_row"
expect "nor placed (422)" "$(arrange "$fork" '{"place":"top"}')" "422"
expect "nesting by a request is not supported (422)" "$(arrange "$t3" '{"nested":true}')" "422"
expect "ejecting is 200" "$(arrange "$fork" '{"nested":false}')" "200"
expect "it is nested under nobody now and still says where it was forked from" \
  "$(jq -r '[has("nestedUnder"), .forkedFrom.threadId == $p] | join(" ")' --arg p "$t2" "$tmp/body")" "false true"
expect "it follows the block it left" "$(order_of '&order=rail')" "$t1,$t2,$fork,$t3"
expect "and moves alone: t2 after t3 no longer carries it" \
  "$(arrange "$t2" "$(jq -n --arg id "$t3" '{place: {after: $id}}')") $(order_of '&order=rail')" "200 $t1,$fork,$t3,$t2"
expect "ejecting a thread that is nobody's child is 200 and changes nothing" \
  "$(arrange "$t2" '{"nested":false}') $(order_of '&order=rail')" "200 $t1,$fork,$t3,$t2"

echo "== none of it is in the log, and nobody else can do it"
api GET "/api/threads/$t1/export" >"$tmp/export-after.json" || true
expect "the events of the pinned thread are what they were" \
  "$(jq -c '[.events[] | [.seq, .kind]]' "$tmp/export-after.json")" "$events_before"
expect "its lastSeq is what it was" "$(api GET "/api/threads/$t1" | jq -r .lastSeq)" "$last_seq"
expect "and its updatedAt" "$(api GET "/api/threads/$t1" | jq -r .updatedAt)" "$updated_at"
expect "the export says what the owner did to their list" "$(jq -r '.thread.pinned' "$tmp/export-after.json")" "true"
expect "another person's request (the administrator's) is a 404" \
  "$(call "$other_header" PATCH "/api/threads/$t1/rail" '{"pinned":false}')" "404"
expect "and changed nothing" "$(api GET "/api/threads/$t1" | jq -r .pinned)" "true"
expect "a thread that is not there is a 404" "$(arrange "$(uuid)" '{"pinned":true}')" "404"
expect "an order that is neither is 400" "$(call "$id_header" GET '/api/threads?order=sideways')" "400"
expect "an archived filter that is none is 400" "$(call "$id_header" GET '/api/threads?archived=maybe')" "400"

finish
