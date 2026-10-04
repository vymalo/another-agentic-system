#!/usr/bin/env sh
# System-level test of deleting a thread (ADR 0043): `DELETE /api/threads/{id}` erases the thread, its log, its
# files and its links, keeps its forks, is refused while the thread works, and is the owner's alone.
#
#   dev/delete-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; the `chat` agent runs from the
# same image):
#
#   docker compose --profile app up -d --build --wait
#
# The agents. `chat` (`adam-agent` on the scripted `mock-persona`) is the agent of the thread that works: its
# `[mock:slow]` script (dev/wiremock/model/mappings/persona-slow*.json) keeps a task `working` for twenty seconds, as
# dev/steer-e2e.sh uses it. `coder-share` (dev/agents.yaml, the coder with no gate, on the model script `[mock:share]` of
# dev/wiremock/coder-share, as dev/artifact-e2e.sh uses it) is the agent of the thread that has files: it makes an SVG,
# a PNG and a JSON file and shares them, so the orchestrator keeps three files under `threads/<thread>/` of its artifact
# store (the named volume `orchestrator-artifacts`, ADR 0032). The script speaks AG-UI to start a run, as the web does
# (docs/api/agui.md), and the resource API for the rest (docs/api/chat-api.yaml): `deleteThread`, `cancelThread`,
# `forkThread`, `shareThread`, `getSharedThread`, `getThread`, `exportThread`, `getArtifact` and `listThreads`.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * ACTIVE: a thread that works (`chat`, `[mock:slow]`, state `working`) is refused with 409 and `code: thread_active`,
#     and is untouched (still `working`, still listed); cancelled (`POST /api/threads/{id}/cancel`), it reaches
#     `cancelled` when the agent reports the stop, and then the same request is 204 and the thread is a 404;
#   * ANOTHER USER: somebody else's delete of the thread that has files (a user of the mock issuer, and the
#     administrator) is 404 and changes nothing: the thread still reads, its link still opens, its files are still in the
#     volume;
#   * FILES, LINK, FORK: the `coder-share` thread ends `done` with three files (a directory of three in the volume, and
#     one under the fork's own directory, which the fork copied when it was made), has a fork (`forkThread`) and a link
#     (`shareThread`, `internal`, opened by another user: 200). The owner's DELETE is 204. Then: the thread is a 404 for
#     `getThread` and `exportThread`; the link is a 404 for the other user and for the owner (the nonce went with the
#     row); the thread's directory is gone from the volume and its file is a 404 through the API; the FORK survives: it
#     reads, says it was forked from a thread that is gone (`forkedFrom` has no `threadId`, and keeps its `kind` and
#     `seq`), is in the list on the list's own level, has all its events, and serves its own copy of the file with the
#     bytes' hash; a second DELETE is 404; the metrics say the delete (`threads_deleted_total`) and that nothing is left
#     to purge (`thread_purges_pending 0`);
#   * LATE REPLY: a thread on `chat` whose first message holds `[mock:title-slow]` ends `done`, and the model that titles it
#     (`mock-title`, which answers a conversation that holds that word after six seconds) is asked; the thread is deleted
#     while the title is on its way (204). When the answer arrives it finds no thread: the orchestrator drops it, counts
#     it (`late_input_dropped_total{source="dispatcher"}` went up), is still ready and serves a new thread to its end.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL      dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   OTHER_EMAIL     someone-else@example.com, another user, who must not be able to delete the first one's thread or to open
#                   its link (a user of dev/mock-oidc/users.json)
#   ADMIN_EMAIL     admin@example.com, the administrator, whose delete of another person's thread is a 404 as well
#   ORCH_SERVICE    orchestrator, the compose service that serves the API and mounts the artifact volume
#   FILES_DIR       dev/wiremock/coder-share/files, the three files the script makes
#   CATALOG_FILE    web/src/features/chat/lib/a2ui/catalog/catalog.json        the screen's catalog
#   CATALOG_LOCK    web/src/features/chat/lib/a2ui/catalog/catalog.lock.json   its {version, digest}
#   TIMEOUT         120    seconds to wait for a thread to stop
#
# It reads the artifact volume with a throwaway container of the image of the `postgres` service (it has `ls`; the
# orchestrator's own image is distroless and has no shell) and the metrics through `docker compose exec edge wget`, as
# dev/split-e2e.sh does, so it needs docker and a stack started with docker compose in this directory's project. It
# deletes the threads it makes (and leaves the fork of the `coder-share` thread, which is a thread of the user's list like
# any other scenario's). It takes about a minute: the `[mock:slow]` task is stopped, the title is waited for. Needs curl and
# jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml; the stack could not be run
# where this script was written, so it has been read and shellchecked, not run.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
other_email=${OTHER_EMAIL:-someone-else@example.com}
admin_email=${ADMIN_EMAIL:-admin@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$here/auth-header.sh" "$email")
other_header=$(sh "$here/auth-header.sh" "$other_email")
admin_header=$(sh "$here/auth-header.sh" "$admin_email")
service=${ORCH_SERVICE:-orchestrator}
files_dir=${FILES_DIR:-$root/dev/wiremock/coder-share/files}
catalog_file=${CATALOG_FILE:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.json}
catalog_lock=${CATALOG_LOCK:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.lock.json}
timeout=${TIMEOUT:-120}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "delete e2e passed"; else echo "delete e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
background=""
cleanup() {
  for _pid in $background; do kill "$_pid" 2>/dev/null || true; done
  rm -rf "$tmp"
}
trap cleanup EXIT

for tool in curl jq docker; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done
for f in "$catalog_file" "$catalog_lock" "$files_dir/chart.svg" "$files_dir/square.png" "$files_dir/report.json"; do
  [ -f "$f" ] || { echo "FAIL $f does not exist (CATALOG_FILE, CATALOG_LOCK and FILES_DIR name the catalog, its lock and the files)"; exit 1; }
done

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API, as the user)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
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

wait_done() { # wait_done THREAD: the thread reaches a state in which its turn is over
  _n=0
  while :; do
    case $(state_of "$1") in done | blocked | failed | cancelled) return 0 ;; esac
    _n=$((_n + 1))
    [ "$_n" -lt $((timeout * 2)) ] || return 1
    sleep 0.5
  done
}

run_input() { # run_input THREAD TEXT [VERSION DIGEST CATALOG]: the RunAgentInput of a first message, with the screen's catalog
  if [ $# -lt 5 ]; then
    jq -nc --arg thread "$1" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [],
      messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}'
  else
    jq -c --arg thread "$1" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$2" \
      --argjson version "$3" --arg digest "$4" '{
        threadId: $thread, runId: $run, state: {}, tools: [], context: [],
        messages: [{id: $msg, role: "user", content: $text}],
        forwardedProps: {"vymalo.uiCatalog": {catalogId: .catalogId, version: $version, digest: $digest, catalog: .}}}' "$5"
  fi
}

post_run() { # post_run AGENT INPUT_FILE OUT_FILE: one run, to its end; prints the HTTP status
  curl -sS -N --max-time "$timeout" -o "$3" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary "@$2" 2>"$tmp/err" || true
}

# --- the stack -------------------------------------------------------------------------------------------------------------
agents=$(api GET /api/agents | jq -r '[.[].id] | join(" ")')
for a in chat coder-share; do
  case " $agents " in
    *" $a "*) ;;
    *) echo "the agent '$a' is not listed by GET /api/agents: is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
  esac
done
if [ "$(api GET /api/me | jq -r '[.permissions[].permission] | index("thread.delete") != null')" != true ]; then
  echo "GET /api/me does not list thread.delete for $email: is dev/orchestrator.yaml the one of ADR 0043 (the roles list it)?" >&2
  exit 2
fi
if [ "$(api GET /api/me | jq -r '.sharing // "disabled"')" = disabled ]; then
  echo "sharing is disabled for $email (GET /api/me): dev/orchestrator.yaml has a \`sharing\` section and the roles hold thread.share" >&2
  exit 2
fi

# The artifact volume, read by a throwaway container (the orchestrator's image is distroless).
container=$(docker compose ps -q "$service" 2>/dev/null | head -n 1)
volume=
if [ -n "$container" ]; then
  volume=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/var/lib/orchestrator/artifacts"}}{{.Name}}{{end}}{{end}}' "$container" 2>/dev/null || true)
fi
helper=$(docker inspect --format '{{.Config.Image}}' "$(docker compose ps -q postgres 2>/dev/null | head -n 1)" 2>/dev/null || true)
if [ -z "$volume" ] || [ -z "$helper" ]; then
  echo "cannot find the artifact volume of the compose service '$service' (ORCH_SERVICE) or an image to read it with: is the stack started with docker compose from this repository?" >&2
  exit 2
fi
volume_ls() { # volume_ls DIR: the names under DIR of the artifact root, sorted, one per line; nothing when it is not there
  docker run --rm -v "$volume:/a:ro" --entrypoint ls "$helper" "/a/$1" 2>/dev/null | sort || true
}
metric() { # metric NAME: the value of a sample of the orchestrator's /metrics (read from inside the stack, as split-e2e.sh does)
  docker compose exec -T edge wget -qO- "http://$service:8080/metrics" 2>/dev/null |
    awk -v name="$1" '$1 == name { print $2; found = 1 } END { if (!found) print "none" }'
}

# --- ACTIVE: a thread that works is refused --------------------------------------------------------------------------------
echo "== a thread that works is refused, and goes when it has ended"
slow=$(uuid)
run_input "$slow" '[mock:slow] Refactor the parser, and take your time.' >"$tmp/slow.json"
(post_run chat "$tmp/slow.json" "$tmp/slow.sse" >"$tmp/slow.code") &
background="$background $!"
if wait_state "$slow" working; then ok "the thread works (the agent said so, and the task takes twenty seconds)"; else bad "the thread never reached working (it is '$(state_of "$slow")')"; fi
expect "DELETE of a thread that works is 409" "$(call "$id_header" DELETE "/api/threads/$slow")" 409
expect "it says thread_active, a code the web acts on" "$(jq -r .code "$tmp/body")" thread_active
expect "the thread is untouched: it still works" "$(state_of "$slow")" working
expect "and is still listed" \
  "$(api GET '/api/threads?limit=100' | jq -r --arg id "$slow" '[.[] | select(.id == $id)] | length')" 1
expect "stopping it is accepted (202)" "$(call "$id_header" POST "/api/threads/$slow/cancel")" 202
if wait_state "$slow" cancelled; then ok "the thread is cancelled when the agent reports the stop"; else bad "the thread did not reach cancelled (it is '$(state_of "$slow")')"; fi
expect "DELETE of the cancelled thread is 204" "$(call "$id_header" DELETE "/api/threads/$slow")" 204
expect "the thread is a 404 now" "$(call "$id_header" GET "/api/threads/$slow")" 404

# --- a thread with files, a fork and a link ----------------------------------------------------------------------------------
echo "== a thread with three files, a fork and a link"
version=$(jq -r '.version' "$catalog_lock")
digest=$(jq -r '.digest' "$catalog_lock")
thread=$(uuid)
run_input "$thread" '[mock:share] make me a small chart as an SVG, a picture and a report, and show them to me' \
  "$version" "$digest" "$catalog_file" >"$tmp/share.json"
expect "the run is accepted" "$(post_run coder-share "$tmp/share.json" "$tmp/share.sse")" 200
if wait_state "$thread" "done"; then ok "the thread ends done"; else bad "the thread did not end done (it is '$(state_of "$thread")')"; fi
api GET "/api/threads/$thread/export" >"$tmp/export.json" || echo '{"events":[]}' >"$tmp/export.json"
shas=$(jq -r '[.events[] | select(.kind == "artifact" and .data.file != null) | .data.file.sha256] | unique | .[]' "$tmp/export.json")
expect "the log has three files" "$(printf '%s\n' "$shas" | grep -c .)" 3
first_sha=$(printf '%s\n' "$shas" | head -n 1)
expect "the volume holds the three files of the thread (and a meta file beside each of the directory store's)" \
  "$(volume_ls "threads/$thread" | grep -Ec '^[0-9a-f]{64}$')" 3
fork=$(uuid)
# The cut is the end of the turn that holds `after`, and the screen's catalog is the log's first event (a `ui_catalog`,
# recorded before the message), so `after: 1` would end the turn before the person's message and copy no file: cut at the
# last event of the log, where the three files are.
last_seq=$(jq -r '[.events[].seq] | max // 0' "$tmp/export.json")
expect "forking the thread is 201" \
  "$(call "$id_header" POST "/api/threads/$thread/fork" "$(jq -n --arg id "$fork" --argjson after "$last_seq" '{after: $after, id: $id}')")" 201
fork_cut=$(jq -r '.forkedFrom.seq' "$tmp/body")
expect "the fork copied the files into its own directory" \
  "$(volume_ls "threads/$fork" | grep -Ec '^[0-9a-f]{64}$')" 3
expect "the fork is nested under its parent" "$(api GET "/api/threads/$fork" | jq -r '.nestedUnder == $p' --arg p "$thread")" true
expect "sharing the thread is 200" "$(call "$id_header" PUT "/api/threads/$thread/share" '{"visibility":"internal"}')" 200
token=$(jq -r '.url // empty | sub("^.*/s/"; "")' "$tmp/body")
if [ -n "$token" ]; then ok "the link has a token"; else bad "the share has no link: $(head -c 300 "$tmp/body")"; fi
expect "another user opens the link: 200" "$(call "$other_header" GET "/api/shared/$token")" 200

echo "== somebody else's delete is a 404 and changes nothing"
expect "another user's DELETE is 404" "$(call "$other_header" DELETE "/api/threads/$thread")" 404
expect "the administrator's DELETE is 404" "$(call "$admin_header" DELETE "/api/threads/$thread")" 404
expect "the thread still reads" "$(call "$id_header" GET "/api/threads/$thread")" 200
expect "its link still opens" "$(call "$other_header" GET "/api/shared/$token")" 200
expect "its files are still in the volume" "$(volume_ls "threads/$thread" | grep -Ec '^[0-9a-f]{64}$')" 3

echo "== the owner deletes it"
before=$(metric threads_deleted_total)
expect "DELETE is 204" "$(call "$id_header" DELETE "/api/threads/$thread")" 204
expect "no body" "$(wc -c <"$tmp/body" | tr -d ' ')" 0
expect "the thread is a 404" "$(call "$id_header" GET "/api/threads/$thread")" 404
expect "and so is its export" "$(call "$id_header" GET "/api/threads/$thread/export")" 404
expect "the link is a 404 for the user it was shown to" "$(call "$other_header" GET "/api/shared/$token")" 404
expect "and for the owner" "$(call "$id_header" GET "/api/shared/$token")" 404
expect "the thread's directory is gone from the volume" "$(volume_ls "threads/$thread" | wc -l | tr -d ' ')" 0
expect "and its file is a 404 through the API" "$(call "$id_header" GET "/api/threads/$thread/artifacts/$first_sha")" 404
expect "a second DELETE is 404" "$(call "$id_header" DELETE "/api/threads/$thread")" 404

echo "== the fork survives, whole"
expect "the fork reads (200)" "$(call "$id_header" GET "/api/threads/$fork")" 200
cp "$tmp/body" "$tmp/fork.json"
expect "it says it was forked from a thread that is gone (no threadId; the kind and the cut stay)" \
  "$(jq -r '[(.forkedFrom | has("threadId")), .forkedFrom.kind, (.forkedFrom.seq == ($seq | tonumber))] | join(" ")' --arg seq "$fork_cut" "$tmp/fork.json")" "false fork true"
expect "it is on the list's own level now" "$(jq -r 'has("nestedUnder")' "$tmp/fork.json")" false
expect "and in the list" \
  "$(api GET '/api/threads?limit=100&order=rail' | jq -r --arg id "$fork" '[.[] | select(.id == $id)] | length')" 1
expect "it has every event it copied" \
  "$(api GET "/api/threads/$fork/export" | jq -r '[.events[] | select(.kind == "artifact")] | length')" 3
expect "its own copy of the file is served (200)" "$(call "$id_header" GET "/api/threads/$fork/artifacts/$first_sha")" 200
expect "and is the file (the hash is the key)" \
  "$(if command -v sha256sum >/dev/null 2>&1; then sha256sum <"$tmp/body"; else shasum -a 256 <"$tmp/body"; fi | cut -d ' ' -f 1)" "$first_sha"
expect "the fork's files are still in the volume" "$(volume_ls "threads/$fork" | grep -Ec '^[0-9a-f]{64}$')" 3
after=$(metric threads_deleted_total)
if [ "$before" != none ] && [ "$after" != none ] && [ "$after" -ge $((before + 1)) ]; then
  ok "threads_deleted_total went up ($before to $after)"
else
  bad "threads_deleted_total did not go up (was '$before', is '$after')"
fi
expect "nothing is left to purge (thread_purges_pending)" "$(metric thread_purges_pending)" 0

# --- LATE REPLY: the model answers for a thread that is gone ---------------------------------------------------------------
echo "== a model's late reply for a deleted thread is dropped"
late=$(uuid)
late_before=$(metric 'late_input_dropped_total{source="dispatcher"}')
run_input "$late" '[mock:title-slow] hello there' >"$tmp/late.json"
expect "the run is accepted" "$(post_run chat "$tmp/late.json" "$tmp/late.sse")" 200
if wait_state "$late" "done"; then ok "the thread ends done (its title is on its way: the model takes six seconds)"; else bad "the thread did not end done"; fi
# the title is asked for right after the first reply: delete now, while the model is still thinking
expect "DELETE is 204" "$(call "$id_header" DELETE "/api/threads/$late")" 204
sleep 10
late_after=$(metric 'late_input_dropped_total{source="dispatcher"}')
if [ "$late_before" != none ] && [ "$late_after" != none ] && [ "$late_after" -ge $((late_before + 1)) ]; then
  ok "the late title was dropped and counted (late_input_dropped_total $late_before to $late_after)"
else
  bad "the late title was not counted (late_input_dropped_total was '$late_before', is '$late_after')"
fi
expect "the orchestrator is ready" "$(curl -sS -o /dev/null -w '%{http_code}' --max-time 30 "$base/readyz" || true)" 200
again=$(uuid)
run_input "$again" 'hello again' >"$tmp/again.json"
expect "a new thread is accepted" "$(post_run chat "$tmp/again.json" "$tmp/again.sse")" 200
if wait_state "$again" "done"; then ok "and served to its end"; else bad "the new thread did not end done (it is '$(state_of "$again")')"; fi
expect "the deleted thread is still a 404" "$(call "$id_header" GET "/api/threads/$late")" 404

for _pid in $background; do wait "$_pid" 2>/dev/null || true; done
finish
