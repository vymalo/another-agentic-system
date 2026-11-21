#!/usr/bin/env sh
# Reading a run to its end, as a client must since ADR 0082: the orchestrator ends every SSE stream after the
# deployment's lifetime (60 s by default), whatever the credential, and nothing of the log goes with it. A stream
# that ends is not a run that ended: the client reopens the run from the last `id:` it saw, and sees each later
# event once.
#
# A script of this directory that reads a run to its end sources this file and calls `sse_run_to_end` after the
# POST whose response it saved:
#
#   # shellcheck disable=SC1091   (the path is the script's own directory: shellcheck does not follow it)
#   . "$(dirname "$0")/sse.sh"
#   code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST ...)
#   if [ "$code" = 200 ]; then
#     sse_run_to_end "$base" "$thread" "$tmp/run.sse" "$id_header" "$timeout"
#   fi
#
# It reopens `GET /agui/threads/{id}/connect?mode=run` with `Last-Event-ID: <the last id: the file holds>`
# (docs/api/agui.md, "Resume points"), appends the answer to the same file, and repeats until the file holds
# `RUN_FINISHED` or `RUN_ERROR`, or TIMEOUT passes. What it leaves is what one uninterrupted stream would have
# written: every frame once, in order.
#
# An append alone would not do, for two reasons the contract gives (docs/api/agui.md, "Resume points" and
# "Where the log ends"):
#   * `id:` is written on the last frame of a log event only, and never while a text message of the log is open,
#     so what a cut stream wrote after its last `id:` is not a resume point. The reopen says it again from the
#     cursor, so it is dropped first — the web client does the same (`connectLoop` discards `pending`).
#   * a reopen replays from the cursor and then re-opens the run that is open there with a *preamble*:
#     `RUN_STARTED`, `SUBAGENT_STARTED` for the open invocation, `STATE_SNAPSHOT`, none of which carries an
#     `id:`. A client that already holds the run drops the ones it holds, or a check that counts a run's frames
#     ("one run, ended once") counts the opening twice. The newer `STATE_SNAPSHOT` stays: it is the state now.
#
# Needs curl, jq and sed. POSIX sh; source it, do not run it.

# sse_terminal FILE: whether the run saved in FILE ended (a `RUN_FINISHED` or a `RUN_ERROR` frame).
sse_terminal() {
  sed -n 's/^data: *//p' "$1" 2>/dev/null |
    jq -R 'fromjson? // empty' 2>/dev/null |
    jq -se 'any(.[]? | select(type == "object") | .type; . == "RUN_FINISHED" or . == "RUN_ERROR")' >/dev/null 2>&1
}

# sse_last_id FILE: the last resume point FILE holds (the `id:` of its last frame), empty when it holds none.
sse_last_id() {
  sed -n 's/^id: *//p' "$1" 2>/dev/null | tail -n 1
}

# sse_truncate_to_cursor FILE: keep FILE up to and including its last frame that carries an `id:`, drop what the
# stream wrote after it. The reopen replays from that cursor, so nothing is lost, and a stream cut in the middle
# of a line loses only that line.
sse_truncate_to_cursor() {
  _sse_file=$1
  if jq -R -s -r '
      split("\n\n") | map(select(test("\\S"))) as $blocks
      | ([range(0; $blocks | length) | select($blocks[.] | test("(?m)^id: *[0-9]+ *$"))] | last) as $k
      | (if $k == null then "" else ($blocks[0:($k + 1)] | join("\n\n")) + "\n\n" end)
    ' "$_sse_file" >"$_sse_file.cut" 2>/dev/null; then
    mv "$_sse_file.cut" "$_sse_file"
  else
    rm -f "$_sse_file.cut"
  fi
}

# sse_compact FILE: FILE with each frame once — a frame whose `id:` is already there is dropped, and so is a
# `RUN_STARTED` or `SUBAGENT_STARTED` whose run is (the preamble of a reopen, which re-opens the run the client
# holds). Everything else, in order.
sse_compact() {
  _sse_file=$1
  if jq -R -s -r '
      split("\n\n") | map(select(test("\\S")))
      | reduce .[] as $block ({ids: {}, runs: {}, out: []};
          ($block | [scan("(?m)^id: *([0-9]+) *$")] | last | if . == null then null else .[0] end) as $id
          | ($block | [scan("(?m)^data: *(.*)$")] | last | if . == null then null else (.[0] | fromjson? // null) end
              | if type == "object" then . else null end) as $event
          | (if $event == null then null
             elif $event.type == "SUBAGENT_STARTED" then $event.subagentRunId
             else $event.runId end) as $run
          | (if $event != null and ($event.type == "RUN_STARTED" or $event.type == "SUBAGENT_STARTED")
             then true else false end) as $opens
          | if $id != null and (.ids[$id] // false) then .
            elif $opens and $run != null and (.runs[$run] // false) then .
            else .out += [$block]
              | (if $id != null then .ids[$id] = true else . end)
              | (if $opens and $run != null then .runs[$run] = true else . end)
            end)
      | .out | join("\n\n") | if . == "" then "" else . + "\n\n" end
    ' "$_sse_file" >"$_sse_file.compact" 2>/dev/null; then
    mv "$_sse_file.compact" "$_sse_file"
  else
    rm -f "$_sse_file.compact"
  fi
}

# sse_run_to_end BASE THREAD FILE HEADER TIMEOUT: read the run of THREAD in FILE to its end, reopening it as a
# stream that ends must be. The POST whose body is in FILE must have answered 200. Prints nothing; FILE is the
# run's frames, each once, from the run's start to its terminal event when one came within TIMEOUT seconds.
sse_run_to_end() {
  _sse_base=$1
  _sse_thread=$2
  _sse_file=$3
  _sse_header=$4
  _sse_timeout=$5
  _sse_deadline=$(( $(date +%s) + _sse_timeout ))
  _sse_reopened=0
  _sse_quiet=0
  while ! sse_terminal "$_sse_file"; do
    if [ "$(date +%s)" -ge "$_sse_deadline" ]; then
      break
    fi
    _sse_cursor=$(sse_last_id "$_sse_file" || true)
    if [ -z "$_sse_cursor" ]; then
      # No resume point: the POST wrote none (a refusal, or a stream cut before its first frame). Nothing to reopen.
      break
    fi
    _sse_code=$(curl -sS -N --max-time "$_sse_timeout" -o "$_sse_file.reopen" -w '%{http_code}' \
      -H "$_sse_header" -H 'accept: text/event-stream' -H "Last-Event-ID: $_sse_cursor" \
      "$_sse_base/agui/threads/$_sse_thread/connect?mode=run" 2>/dev/null || true)
    if [ "$_sse_code" != 200 ]; then
      # 404 (the thread is gone) or a refusal: no reopen will say the rest of this run, and what the stream wrote
      # is left as it is.
      rm -f "$_sse_file.reopen"
      break
    fi
    _sse_reopened=1
    _sse_bytes=$(wc -c <"$_sse_file.reopen" 2>/dev/null | tr -d ' ' || true)
    # What the cut stream wrote after its last resume point is not a resume point: the reopen says it again.
    sse_truncate_to_cursor "$_sse_file"
    cat "$_sse_file.reopen" >>"$_sse_file"
    rm -f "$_sse_file.reopen"
    if [ "${_sse_bytes:-0}" -eq 0 ]; then
      # The stream ended at once: no run is open at the cursor and the log holds nothing after it. Three of those
      # and the log is not going to say a terminal event (a cancelled or dropped run).
      _sse_quiet=$(( _sse_quiet + 1 ))
      if [ "$_sse_quiet" -ge 3 ]; then
        break
      fi
      sleep 1
    else
      _sse_quiet=0
    fi
  done
  if [ "$_sse_reopened" = 1 ]; then
    sse_compact "$_sse_file"
  fi
}
