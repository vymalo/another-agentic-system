#!/usr/bin/env sh
# System-level test of a file handed from an agent to a person (ADR 0032, plan 10 section 3.3): the coder makes three files in a
# scratch project, shares each with `share_file` (an A2A artifact with a `raw` part), the orchestrator keeps them in its artifact
# store and the thread's log holds only references, the coder then places two of them in a surface with the catalog's `Image`, and
# the API serves the bytes to the thread's owner and to nobody else.
#
#   dev/artifact-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# Why the coder, and why its own entry. `share_file` is a tool of adam-coder (adam-rs ADR 0012: it reads one file of the coder's
# workspace and returns it as a file artifact); the agents that are only a folder (`chat`, `researcher`, served by adam-agent) have no
# workspace and no such tool, and an agent that is not adam would need a script of its own on a WireMock A2A mock that cannot make
# bytes. The coder's gate (`coder` in dev/agents.yaml) wants the checks of a pushed commit and a green CI report, and this task
# pushes nothing (the person asked for a result, not for a change to a repository), so it runs as `coder-share`: the same coder, no
# gate. Its model is `mock-coder` and the keyword `[mock:share]` selects the script (dev/wiremock/coder-share, ours, not vendored;
# `dev/check-agent-mocks.sh` plays it). The script, one model call each: `start_scratch`; `write_file` chart.svg (an SVG with a
# script and two event handlers in it, on purpose); `write_file` report.json; `run` (makes square.png from base64, a binary file the
# model cannot write as text); `share_file` for each of the three; `ui_catalog`; `show` (a Text and two `Image`s, the SVG and the PNG,
# by the sha256 of the files); then the answer. The three files are dev/wiremock/coder-share/files/, the one source of the bytes
# (the script embeds them, and check-agent-mocks.sh asserts it does).
#
# The script speaks AG-UI, as the web does (docs/api/agui.md): one POST /agui/agents/coder-share per run, the thread's state from
# GET /api/threads/{id}, its frames from GET /agui/threads/{id}/connect?mode=run, its log from GET /api/threads/{id}/export. The
# catalog it sends is the one the web ships (version 4 or later has `Image`), read from
# web/src/features/chat/lib/a2ui/catalog/catalog.json and catalog.lock.json (`forwardedProps["vymalo.uiCatalog"]`).
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the shipped catalog has an `Image`, and `coder-share` is listed by GET /api/agents;
#   * the run ends RUN_FINISHED (success) and the thread `done` (no gate, no pull request), with no `error` in its log;
#   * KEPT: the thread has exactly three file artifacts, `Chart`, `Square` and `Report`, in that order, in the AG-UI frames
#     (`vymalo.artifact`) and in the log (`artifact`). Each frame says `kind: "file"`, `href` equal to
#     /api/threads/<thread>/artifacts/<sha256>, the same hash in `sha256` (and it is the SHA-256 of the fixture file here: the
#     orchestrator hashed the bytes the coder made, and it did not alter them), the size of the fixture, its file name and the
#     media type it sniffed (image/svg+xml, image/png, application/json) and its `preview` (image, image, text); the log's
#     entry holds the reference and no bytes;
#   * ORDER: every file the surface places is in the thread before the surface is (the frame of its file comes first), and
#     the surface (`a2ui-surface`, under the screen's catalogId) holds a Text and two `Image`s whose `artifact` are the hashes of the SVG
#     and the PNG, with an alt each, and no Image of the JSON; the model's `show` was accepted ("Shown to the person.") and each
#     `share_file` was answered "Shared <file> (<size> bytes, <type>).";
#   * SERVED: GET href (through the edge, which adds the identity) answers 200 with the type the worker kept, `nosniff`,
#     the sandboxing Content-Security-Policy, `inline` for these preview types, an immutable private cache and an ETag of the
#     hash; the PNG's and the JSON's bytes hash to `sha256`; the SVG inline is SANITIZED (no `<script`, no `on*` attribute, none of the
#     coder's markers, its shapes kept, so its hash is not the file's) and with `?download=1` the ORIGINAL bytes (the hash is the
#     file's, an attachment named after the file, the script still in it); a download of the others equals the file too;
#   * NOT SERVED: another user's request for the same href is a 404 (through the
#     edge with a token of the mock issuer for that user, a control request of the owner's beside it), and so is the
#     same hash under a thread that is the same person's but holds no such file, a thread that does not exist, a hash the thread
#     holds no file for, and a hash that is not 64 lowercase hex digits; a bad `download` value is a 400;
#   * the coder's model mock answered every request (no error, none unmatched).
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   OTHER_EMAIL      someone-else@example.com, the other user of the 404 check (a user of dev/mock-oidc/users.json)
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}, the coder's model
#   CATALOG_FILE     web/src/features/chat/lib/a2ui/catalog/catalog.json        the screen's catalog
#   CATALOG_LOCK     web/src/features/chat/lib/a2ui/catalog/catalog.lock.json   its {version, digest}
#   FILES_DIR        dev/wiremock/coder-share/files   the three files the script makes
#   TIMEOUT          120    seconds to wait for a run to end
#
# It EMPTIES the request journal of `mock-openai` first, so run it on a stack you are not in the middle of another scenario on. Needs
# curl, jq and sha256sum (or shasum), and /proc or uuidgen for a UUID. It runs against the `split` profile too (a worker keeps the file, the control plane serves it: the two
# must see the same directory). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
other_email=${OTHER_EMAIL:-someone-else@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
other_header=$(sh "$(dirname "$0")/auth-header.sh" "$other_email")
openai=${MOCK_OPENAI_URL:-http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}}
openai=${openai%/}
timeout=${TIMEOUT:-120}

root=$(cd "$(dirname "$0")/.." && pwd)
catalog_file=${CATALOG_FILE:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.json}
catalog_lock=${CATALOG_LOCK:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.lock.json}
files_dir=${FILES_DIR:-$root/dev/wiremock/coder-share/files}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "artifact e2e passed"; else echo "artifact e2e FAILED"; exit 1; fi
}

for f in "$catalog_file" "$catalog_lock" "$files_dir/chart.svg" "$files_dir/square.png" "$files_dir/report.json"; do
  if [ ! -f "$f" ]; then
    echo "FAIL $f does not exist (CATALOG_FILE and CATALOG_LOCK name the screen's catalog and its lock, FILES_DIR the files the script makes)"
    exit 1
  fi
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, one JSON per line
  sed -n 's/^data: *//p' "$1"
}

sha256_hex() { # the SHA-256 of stdin, as 64 lower-case hex digits
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d ' ' -f 1; else shasum -a 256 | cut -d ' ' -f 1; fi
}

size_of() { wc -c < "$1" | tr -d ' '; }

# --- the files the coder makes, and what the thread must say about each ---------------------------------------------------
svg_sha=$(sha256_hex < "$files_dir/chart.svg")
png_sha=$(sha256_hex < "$files_dir/square.png")
json_sha=$(sha256_hex < "$files_dir/report.json")
svg_size=$(size_of "$files_dir/chart.svg")
png_size=$(size_of "$files_dir/square.png")
json_size=$(size_of "$files_dir/report.json")
echo "chart.svg $svg_sha ($svg_size bytes), square.png $png_sha ($png_size bytes), report.json $json_sha ($json_size bytes)"

catalog_id=$(jq -r '.catalogId' "$catalog_file")
version=$(jq -r '.version' "$catalog_lock")
digest=$(jq -r '.digest' "$catalog_lock")
if jq -e '.components | has("Image")' "$catalog_file" >/dev/null 2>&1; then
  ok "the screen's catalog ($catalog_id, version $version) has an Image"
else
  bad "the shipped catalog has no Image: this scenario needs catalog version 4 or later"
  finish
fi

# --- the agent -------------------------------------------------------------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  for a in coder-share chat; do
    if printf '%s' "$agents" | jq -e --arg a "$a" 'any(.[]; .id == $a)' >/dev/null 2>&1; then
      ok "GET /api/agents lists $a"
    else
      bad "GET /api/agents does not list $a (agents: $(printf '%s' "$agents" | jq -c '[.[].id]')): is this the app profile of compose.yaml, with dev/agents.yaml?"
      finish
    fi
  done
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi

code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$openai/__admin/requests" || true)
if [ "$code" = 200 ]; then ok "journal reset: $openai"; else bad "journal reset: $openai answered HTTP $code"; fi

# --- one run -----------------------------------------------------------------------------------------------------------------
thread=
n=0

# stream INPUT_FILE LABEL AGENT: POST the RunAgentInput to AGENT and wait for the run to end. Sets
#   outcome   how the run stream ended (success, interrupt, error: <code>, or empty)
#   state     the state the thread ended in
#   said      the words the agent spoke in this run (the assistant messages of its stream), joined
#   events    the file holding every frame of the thread so far (a JSON array)
stream() {
  _deadline=$(( $(date +%s) + timeout ))
  _code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/$3" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary "@$1" 2>"$tmp/err" || true)
  if [ "$_code" != 200 ]; then
    bad "$2: POST /agui/agents/$3 answered HTTP ${_code:-none}: $(head -c 300 "$tmp/err") $(head -c 400 "$tmp/run.sse" 2>/dev/null)"
    finish
  fi
  outcome=$(sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true)
  # The words of each assistant message as a client keeps them (live deltas continue from their offset: docs/api/agui.md, "Live text").
  said=$(sse_events "$tmp/run.sse" | jq -rs 'reduce .[] as $f ({order: [], text: {}};
      if $f.type == "TEXT_MESSAGE_START" and $f.role == "assistant" then
        (if (.text | has($f.messageId)) then . else (.order += [$f.messageId] | .text[$f.messageId] = "") end)
      elif $f.type == "TEXT_MESSAGE_CONTENT" and (.text | has($f.messageId)) then
        .text[$f.messageId] |= ((if $f.metadata["vymalo.live"].offset != null then .[0:$f.metadata["vymalo.live"].offset] else . end) + $f.delta)
      else . end)
    | [.order[] as $id | .text[$id]] | join(" ")' 2>/dev/null || true)
  state=
  while :; do
    state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
    case $state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 2
  done
  events=$tmp/events.json
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
    "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
    echo '[]' > "$events"
  echo "$2: the agent said: ${said:-<nothing>}"
}

# why EVENTS: what the thread said about a failure, to help whoever reads the log.
why() {
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$1" | head -n 20
}

# input_message TEXT [VERSION DIGEST CATALOG_FILE]: the RunAgentInput of a run with one new user message in $thread and,
# with the three more arguments, the catalog the screen sends under forwardedProps["vymalo.uiCatalog"] (the file is read as
# jq's input, so `.` is the catalog).
input_message() {
  n=$((n + 1))
  if [ "$#" -lt 4 ]; then
    jq -nc --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [],
      messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}'
    return
  fi
  jq -c --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" \
    --argjson version "$2" --arg digest "$3" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [],
      messages: [{id: $msg, role: "user", content: $text}],
      forwardedProps: {"vymalo.uiCatalog": {catalogId: .catalogId, version: $version, digest: $digest, catalog: .}}}' "$4"
}

# requests: the bodies and statuses of the requests the coder's model mock got for mock-coder, oldest first (WireMock's journal
# lists the newest first), as a JSON array of {status, body}.
requests() {
  curl -s --max-time 30 "$openai/__admin/requests" |
    jq -c '[.requests | reverse | .[] | {status: .response.status, body: (.request.body | fromjson? // {})} | select(.body.model == "mock-coder")]' 2>/dev/null || echo '[]'
}

# tool_result REQUESTS CALL_ID: what the last request of REQUESTS that holds a tool message for CALL_ID says it was.
tool_result() {
  printf '%s' "$1" | jq -r --arg id "$2" '[.[] | .body.messages // [] | .[] | select(.role == "tool" and .tool_call_id == $id) | .content] | last // empty'
}

thread=$(uuid)
echo "thread $thread"
echo
echo "run: [mock:share] with the screen's catalog (version $version)"
input_message '[mock:share] make me a small chart as an SVG, a picture and a report, and show them to me' "$version" "$digest" "$catalog_file" > "$tmp/run1.json"
stream "$tmp/run1.json" "run" coder-share
if [ "$outcome" = success ]; then
  ok "the run stream ended with RUN_FINISHED (success)"
else
  bad "the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"
fi
if [ "$state" = "done" ]; then
  ok "the thread ended done (no gate, and no pull request was asked for)"
else
  bad "the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"
  why "$events"
fi
case $said in
  *chart.svg*square.png*report.json*) ok "the answer names the three files" ;;
  *) bad "the answer does not name chart.svg, square.png and report.json: '$said'" ;;
esac
if api GET "/api/threads/$thread/export" > "$tmp/export.json" 2>"$tmp/err"; then
  ok "GET /api/threads/{id}/export"
else
  bad "GET /api/threads/$thread/export: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/export.json")"
  echo '{"events":[]}' > "$tmp/export.json"
fi
errors=$(jq -c '[.events[] | select(.kind == "error") | .data.message // .data] | map(tostring | .[0:120])' "$tmp/export.json" 2>/dev/null || echo '?')
if [ "$errors" = '[]' ]; then
  ok "the log holds no error (a file that could not be kept, or one over a limit, would be one)"
else
  bad "the log holds errors: $errors"
fi

# --- KEPT: the frames and the log ---------------------------------------------------------------------------------------------
# One entry per file, by the artifact's message id (a snapshot may be sent again), in the order they came.
frames=$(jq -c '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact")]
  | reduce .[] as $f ([]; if any(.[]; .messageId == $f.messageId) then . else . + [$f] end) | map(.content)' "$events" 2>/dev/null || echo '[]')
count=$(printf '%s' "$frames" | jq -r 'length')
if [ "$count" = 3 ]; then
  ok "the thread's frames hold exactly three file artifacts (vymalo.artifact)"
else
  bad "the thread's frames hold $count vymalo.artifact activities, want 3: $(printf '%s' "$frames" | jq -c 'map([.name, .kind, .sha256 // .uri // "-"])')"
fi
check_frame() { # check_frame NAME FILENAME MIME PREVIEW SHA SIZE
  _f=$(printf '%s' "$frames" | jq -c --arg n "$1" 'map(select(.name == $n)) | first // {}')
  _want="file|/api/threads/$thread/artifacts/$5|$5|$6|$2|$3|$4"
  _got=$(printf '%s' "$_f" | jq -r '[.kind, .href, .sha256, (.size | tostring), .filename, .mimeType, .preview] | map(. // "-") | join("|")')
  if [ "$_got" = "$_want" ]; then
    ok "frame $1: kind file, href /api/threads/<thread>/artifacts/<sha256> with the same hash, $6 bytes, $2, $3, preview $4"
  else
    bad "frame $1: kind|href|sha256|size|filename|mimeType|preview is '$_got', want '$_want'"
  fi
}
check_frame Chart chart.svg image/svg+xml image "$svg_sha" "$svg_size"
check_frame Square square.png image/png image "$png_sha" "$png_size"
check_frame Report report.json application/json text "$json_sha" "$json_size"
if [ "$(printf '%s' "$frames" | jq -r 'map(.name) | join(",")')" = "Chart,Square,Report" ]; then
  ok "the files arrived in the order they were shared (Chart, Square, Report)"
else
  bad "the files arrived as '$(printf '%s' "$frames" | jq -r 'map(.name) | join(",")')', want Chart,Square,Report"
fi

logged=$(jq -c '[.events[] | select(.kind == "artifact" and .data.file != null)]' "$tmp/export.json" 2>/dev/null || echo '[]')
if [ "$(printf '%s' "$logged" | jq -r 'map(.data.name + ":" + .data.file.sha256) | join(",")')" = "Chart:$svg_sha,Square:$png_sha,Report:$json_sha" ]; then
  ok "the log holds exactly three artifact events with a file: the same names and hashes, once each"
else
  bad "the log's artifacts with a file: $(printf '%s' "$logged" | jq -c 'map(.data.name + ":" + .data.file.sha256)'), want Chart:$svg_sha,Square:$png_sha,Report:$json_sha"
fi
# The bytes never reach the log: an artifact event holds a name, a type and the reference, and no content.
if printf '%s' "$logged" | jq -e 'all(.[]; .data.text == null and .data.raw == null and .data.uri == null and ((.data | tojson | length) < 600))' >/dev/null 2>&1; then
  ok "an artifact event of the log is a reference (name, type, sha256, size, file name), with no bytes, text or link"
else
  bad "an artifact event of the log holds more than a reference: $(printf '%s' "$logged" | jq -c '.[0].data' | head -c 400)"
fi

# --- ORDER: the files are in the thread before the surface that places them ---------------------------------------------------
surface_at=$(jq -r '[to_entries[] | select(.value.type == "ACTIVITY_SNAPSHOT" and .value.activityType == "a2ui-surface") | .key] | first // empty' "$events" 2>/dev/null || true)
surfaces=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface") | .messageId] | unique | length' "$events" 2>/dev/null || echo '?')
if [ "$surfaces" = 1 ]; then
  ok "exactly one a2ui-surface"
else
  bad "$surfaces a2ui-surface activities, want exactly one"
fi
for pair in "chart.svg:$svg_sha" "square.png:$png_sha"; do
  file=${pair%%:*}
  sha=${pair#*:}
  at=$(jq -r --arg sha "$sha" '[to_entries[] | select(.value.type == "ACTIVITY_SNAPSHOT" and .value.activityType == "vymalo.artifact" and .value.content.sha256 == $sha) | .key] | first // empty' "$events" 2>/dev/null || true)
  if [ -n "$at" ] && [ -n "$surface_at" ] && [ "$at" -lt "$surface_at" ]; then
    ok "$file is in the thread (frame $at) before the surface that places it (frame $surface_at)"
  else
    bad "$file: its artifact frame is at '${at:-none}' and the surface at '${surface_at:-none}', want the file first"
  fi
done
operations=$(jq -c '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface")] | last | .content.a2ui_operations // []' "$events" 2>/dev/null || echo '[]')
surface_catalog=$(printf '%s' "$operations" | jq -r '[.[] | .createSurface? // empty] | first | .catalogId // empty')
if [ "$surface_catalog" = "$catalog_id" ]; then
  ok "createSurface names the screen's catalog ($catalog_id)"
else
  bad "createSurface names '${surface_catalog:-nothing}', want $catalog_id (operations: $(printf '%s' "$operations" | head -c 400))"
fi
components=$(printf '%s' "$operations" | jq -c '[.[] | .updateComponents? // empty | .components[]?]')
kinds=$(printf '%s' "$components" | jq -r 'map(.component) | join(" ")')
if [ "$(printf '%s' "$components" | jq -r '[.[] | select(.component == "Image") | .artifact] | sort | join(",")')" = "$(printf '%s\n%s\n' "$svg_sha" "$png_sha" | sort | tr '\n' ',' | sed 's/,$//')" ]; then
  ok "the surface holds two Images, of the SVG and of the PNG, by their sha256 (components: $kinds)"
else
  bad "the surface's Images are '$(printf '%s' "$components" | jq -r '[.[] | select(.component == "Image") | .artifact] | join(",")')', want the hashes of chart.svg and square.png (components: ${kinds:-none})"
fi
if printf '%s' "$components" | jq -e '[.[] | select(.component == "Image")] | length == 2 and all(.[]; (.alt | length > 0) and (has("url") | not) and (has("src") | not))' >/dev/null 2>&1; then
  ok "each Image has an alt and names no URL"
else
  bad "an Image has no alt or names a URL: $(printf '%s' "$components" | jq -c '[.[] | select(.component == "Image")]' | head -c 400)"
fi
if printf '%s' "$components" | jq -e 'map(select(.component == "Image" and .artifact == "'"$json_sha"'")) | length == 0' >/dev/null 2>&1; then
  ok "the JSON report is not drawn as an Image (it is only a file)"
else
  bad "the surface places report.json as an Image"
fi

reqs=$(requests)
offered=$(printf '%s' "$reqs" | jq -r '[.[0].body.tools // [] | .[].function.name] | join(" ")')
for tool in start_scratch write_file run share_file ui_catalog show; do
  case " $offered " in
    *" $tool "*) ok "the model was offered $tool" ;;
    *) bad "the model was not offered $tool (tools: ${offered:-none}): is the image pinned in compose.yaml an adam-rs commit with share_file (0e44c14 or later)?" ;;
  esac
done
for pair in "sh-call-5:chart.svg:$svg_size:image/svg+xml" "sh-call-6:square.png:$png_size:image/png" "sh-call-7:report.json:$json_size:application/json"; do
  id=${pair%%:*}
  rest=${pair#*:}
  file=${rest%%:*}
  rest=${rest#*:}
  size=${rest%%:*}
  type=${rest#*:}
  want="Shared $file ($size bytes, $type)."
  got=$(tool_result "$reqs" "$id")
  if [ "$got" = "$want" ]; then
    ok "share_file $file was answered \"$want\""
  else
    bad "the result of share_file $file ($id) is '$(printf '%s' "$got" | head -c 300)', want '$want' (the result of the run that made square.png, sh-call-4: '$(tool_result "$reqs" sh-call-4 | head -c 300)')"
  fi
done
case $(tool_result "$reqs" sh-call-9) in
  *"Shown to the person"*) ok "show was accepted (\"Shown to the person.\"): the screen's catalog took both Images" ;;
  *) bad "the result of show (sh-call-9) is '$(tool_result "$reqs" sh-call-9 | head -c 300)', want \"Shown to the person.\"" ;;
esac
case $(tool_result "$reqs" sh-call-8) in
  *Image*) ok "ui_catalog told the model the screen has Image" ;;
  *) bad "the result of ui_catalog (sh-call-8) names no Image: '$(tool_result "$reqs" sh-call-8 | head -c 300)'" ;;
esac

# --- SERVED --------------------------------------------------------------------------------------------------------------------
csp="default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox"

# header FILE NAME: the value of the response header NAME in a saved head, case-insensitive, trimmed.
header() {
  tr -d '\r' < "$1" | awk -v want="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]')" '
    { k = $0; sub(/:.*/, "", k); if (tolower(k) == want && index($0, ":") > 0) { v = $0; sub(/^[^:]*:[ \t]*/, "", v) } }
    END { print v }'
}

# get PATH_AND_QUERY: GET through the edge as the dev user. Sets $status, and leaves the head in $tmp/head and the body in $tmp/body.
get() {
  status=$(curl -sS --max-time 60 -D "$tmp/head" -o "$tmp/body" -w '%{http_code}' -H "$id_header" "$base$1" 2>"$tmp/err" || true)
}

# serving_headers LABEL SHA TYPE_PREFIX: what every inline response of a file carries, read from the head saved by `get`.
serving_headers() {
  _label=$1
  _sha=$2
  _ct=$(header "$tmp/head" content-type)
  _cd=$(header "$tmp/head" content-disposition)
  _ok=true
  case $_ct in "$3"*) ;; *) _ok=false ;; esac
  if [ "$_ok" = true ]; then ok "$_label: Content-Type is $_ct"; else bad "$_label: Content-Type is '$_ct', want $3"; fi
  if [ "$(header "$tmp/head" x-content-type-options)" = nosniff ]; then ok "$_label: X-Content-Type-Options: nosniff"; else bad "$_label: X-Content-Type-Options is '$(header "$tmp/head" x-content-type-options)', want nosniff"; fi
  if [ "$(header "$tmp/head" content-security-policy)" = "$csp" ]; then ok "$_label: the sandboxing Content-Security-Policy"; else bad "$_label: Content-Security-Policy is '$(header "$tmp/head" content-security-policy)', want '$csp'"; fi
  case $_cd in inline*) ok "$_label: Content-Disposition is $_cd" ;; *) bad "$_label: Content-Disposition is '$_cd', want inline" ;; esac
  case $(header "$tmp/head" cache-control) in
    *private*immutable* | *immutable*private*) ok "$_label: Cache-Control is private and immutable ($(header "$tmp/head" cache-control))" ;;
    *) bad "$_label: Cache-Control is '$(header "$tmp/head" cache-control)', want private, immutable" ;;
  esac
  if [ "$(header "$tmp/head" etag)" = "\"$_sha\"" ]; then ok "$_label: ETag is the hash"; else bad "$_label: ETag is '$(header "$tmp/head" etag)', want \"$_sha\""; fi
}

# The PNG and the JSON: the bytes are the file's.
for spec in "square.png:$png_sha:image/png:$png_size" "report.json:$json_sha:application/json:$json_size"; do
  file=${spec%%:*}
  rest=${spec#*:}
  sha=${rest%%:*}
  rest=${rest#*:}
  type=${rest%%:*}
  size=${rest#*:}
  href=/api/threads/$thread/artifacts/$sha
  get "$href"
  if [ "$status" = 200 ]; then
    ok "GET $file: 200"
    serving_headers "GET $file" "$sha" "$type"
    if [ "$(sha256_hex < "$tmp/body")" = "$sha" ] && [ "$(size_of "$tmp/body")" = "$size" ]; then
      ok "GET $file: the body is $size bytes and its SHA-256 is the hash"
    else
      bad "GET $file: the body is $(size_of "$tmp/body") bytes with SHA-256 $(sha256_hex < "$tmp/body"), want $size bytes and $sha"
    fi
  else
    bad "GET $file: HTTP ${status:-none}, want 200 ($(head -c 200 "$tmp/body" 2>/dev/null) $(head -c 200 "$tmp/err" 2>/dev/null))"
  fi
  get "$href?download=1"
  if [ "$status" = 200 ] && [ "$(sha256_hex < "$tmp/body")" = "$sha" ]; then
    case $(header "$tmp/head" content-disposition) in
      attachment*"filename=\"$file\""*) ok "GET $file?download=1: an attachment named $file, the same bytes" ;;
      *) bad "GET $file?download=1: Content-Disposition is '$(header "$tmp/head" content-disposition)', want an attachment named $file" ;;
    esac
  else
    bad "GET $file?download=1: HTTP ${status:-none} and body SHA-256 $(sha256_hex < "$tmp/body"), want 200 and $sha"
  fi
done

# The SVG: inline it is sanitized, as a download it is what the coder made.
href=/api/threads/$thread/artifacts/$svg_sha
get "$href"
if [ "$status" = 200 ]; then
  ok "GET chart.svg: 200"
  serving_headers "GET chart.svg" "$svg_sha" image/svg+xml
  inline_sha=$(sha256_hex < "$tmp/body")
  if grep -qi '<script' "$tmp/body" || grep -Eqi '[[:space:]]on[a-z]+[[:space:]]*=' "$tmp/body" || grep -q 'mock-xss' "$tmp/body"; then
    bad "GET chart.svg: the inline SVG still holds a script, an event handler or a marker of the coder's: $(head -c 400 "$tmp/body")"
  else
    ok "GET chart.svg: the inline SVG holds no <script, no on* attribute and none of the markers the coder put in"
  fi
  if grep -q '<rect' "$tmp/body" && grep -q '<circle' "$tmp/body" && grep -q '<svg' "$tmp/body"; then
    ok "GET chart.svg: the drawing is kept (svg, rect, circle)"
  else
    bad "GET chart.svg: the sanitized SVG lost its drawing: $(head -c 400 "$tmp/body")"
  fi
  if [ "$inline_sha" != "$svg_sha" ]; then
    ok "GET chart.svg: the inline bytes are not the file's (sanitized, not passed through)"
  else
    bad "GET chart.svg: the inline body is byte for byte the file the coder made, with its script: it was not sanitized"
  fi
else
  bad "GET chart.svg: HTTP ${status:-none}, want 200 ($(head -c 200 "$tmp/body" 2>/dev/null))"
fi
get "$href?download=1"
if [ "$status" = 200 ] && [ "$(sha256_hex < "$tmp/body")" = "$svg_sha" ]; then
  case $(header "$tmp/head" content-disposition) in
    attachment*"filename=\"chart.svg\""*) ok "GET chart.svg?download=1: an attachment named chart.svg, the original bytes (SHA-256 is the hash)" ;;
    *) bad "GET chart.svg?download=1: Content-Disposition is '$(header "$tmp/head" content-disposition)', want an attachment named chart.svg" ;;
  esac
  if [ "$(header "$tmp/head" x-content-type-options)" = nosniff ] && [ "$(header "$tmp/head" content-security-policy)" = "$csp" ]; then
    ok "GET chart.svg?download=1: nosniff and the sandboxing Content-Security-Policy too"
  else
    bad "GET chart.svg?download=1: nosniff is '$(header "$tmp/head" x-content-type-options)', Content-Security-Policy is '$(header "$tmp/head" content-security-policy)'"
  fi
else
  bad "GET chart.svg?download=1: HTTP ${status:-none} and body SHA-256 $(sha256_hex < "$tmp/body"), want 200 and the file's $svg_sha"
fi

# --- NOT SERVED -----------------------------------------------------------------------------------------------------------------
# The same person, other addresses: each is the same 404.
expect_status() { # expect_status WANT DESCRIPTION PATH
  get "$3"
  if [ "$status" = "$1" ]; then ok "$2: $1"; else bad "$2: HTTP ${status:-none}, want $1 ($(head -c 200 "$tmp/body" 2>/dev/null))"; fi
}
zeros=0000000000000000000000000000000000000000000000000000000000000000
expect_status 404 "a hash this thread holds no file for" "/api/threads/$thread/artifacts/$zeros"
expect_status 404 "a thread that does not exist" "/api/threads/$(uuid)/artifacts/$png_sha"
expect_status 404 "a hash in capital letters" "/api/threads/$thread/artifacts/$(printf '%s' "$png_sha" | tr 'a-f' 'A-F')"
expect_status 404 "a hash that is not 64 digits" "/api/threads/$thread/artifacts/$(printf '%s' "$png_sha" | cut -c 1-63)"
expect_status 400 "a download value that is not 0, 1, true or false" "/api/threads/$thread/artifacts/$png_sha?download=maybe"

# Another user: a token of the mock issuer for another user of dev/mock-oidc/users.json, through the edge like the owner's. The owner's own
# request for the same href is the control, so that a failure points at the method and not at the 404.
owner_sha=$(curl -sS --max-time 60 -H "$id_header" "$base/api/threads/$thread/artifacts/$png_sha" 2>"$tmp/err" | sha256_hex || true)
if [ "$owner_sha" = "$png_sha" ]; then
  ok "the owner's request for square.png (the control) is served"
  code=$(curl -sS --max-time 60 -o /dev/null -w '%{http_code}' -H "$other_header" "$base/api/threads/$thread/artifacts/$png_sha" 2>"$tmp/err" || true)
  if [ "$code" = 404 ]; then
    ok "another user ($other_email) asking for the same href gets a 404"
  else
    bad "another user ($other_email) asking for the same href got HTTP ${code:-none}, want 404 ($(head -c 300 "$tmp/err"))"
  fi
  code=$(curl -sS --max-time 60 -o /dev/null -w '%{http_code}' -H "$other_header" "$base/api/threads/$thread/artifacts/$svg_sha?download=1" 2>"$tmp/err" || true)
  if [ "$code" = 404 ]; then
    ok "another user asking for the download of chart.svg gets a 404 too"
  else
    bad "another user asking for the download of chart.svg got HTTP ${code:-none}, want 404"
  fi
else
  bad "the control failed: the owner's request got SHA-256 '$owner_sha', want $png_sha; so the other user's 404 proves nothing ($(head -c 300 "$tmp/err"))"
fi

# The same person's other thread: a hash is no capability, the file belongs to the thread in the path.
thread_a=$thread
thread=$(uuid)
echo
echo "another thread of the same person (chat, \"hi\"): $thread"
input_message 'hi' > "$tmp/run2.json"
stream "$tmp/run2.json" "the other thread" chat
case $state in
  done | blocked) ok "the other thread ended $state" ;;
  *) bad "the other thread ended '${state:-unknown}' (after at most ${timeout}s), want done" ;;
esac
expect_status 404 "the hash of $thread_a's square.png asked of the person's other thread" "/api/threads/$thread/artifacts/$png_sha"

# --- the model's side ------------------------------------------------------------------------------------------------------------
echo
reqs=$(requests)
total=$(printf '%s' "$reqs" | jq -r '[.[] | select(.body | tojson | contains("[mock:share]"))] | length')
errors=$(printf '%s' "$reqs" | jq -r '[.[] | select(.status != 200)] | length')
if [ "$total" -ge 10 ] && [ "$errors" = 0 ]; then
  ok "the coder's model answered all $total requests of the script without an error"
else
  bad "the model mock got $total [mock:share] requests, $errors of them not answered 200 (want at least 10 and no error)"
fi
unmatched=$(curl -s --max-time 30 "$openai/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then
  ok "mock-openai matched every request"
else
  bad "mock-openai saw $unmatched unmatched requests (see $openai/__admin/requests/unmatched)"
fi

finish
