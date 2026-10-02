#!/usr/bin/env sh
# Saves one chat thread as a JSON file, to send to a developer: the same file the web's
# "Export JSON" button downloads (GET /api/threads/{id}/export, docs/api/chat-api.yaml; dev/README.md,
# section "Share a chat with a developer").
#
#   dev/export-thread.sh 0190a1b2-...                 # writes thread-0190a1b2-....json in the current directory
#   dev/export-thread.sh 0190a1b2-... chat.json       # writes chat.json
#   dev/export-thread.sh 0190a1b2-... -               # writes the document to stdout
#
# The thread id is in the address bar of the thread (`/threads/<id>`), and `thread <id>` on the first line
# of dev/try-thread.sh. The file is the whole chat: the thread, its full job (the gate, the attempts, the
# pushed commit, what each check said), the agent binding and every event of the log in order (messages,
# agent statuses, artifacts, the check, CI and verifier cards, reworks). It holds what you and the agents
# wrote, and your e-mail address as the author of your messages; it holds no credential of the
# orchestrator (no bearer token, webhook secret or database URL is ever in the log). Read it before you
# send it: a person can paste anything into a chat.
#
# The script prints the path it wrote and a one-line summary on stderr, and checks the document is what
# it says it is (`format`, `version`, and that the events start at 1 and run without a gap).
#
# Environment (all optional):
#   BASE_URL    where the API is served     (default http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`,
#               oauth2-proxy in front of the API)
#   AUTH_EMAIL  the user (default dev@example.com): a token of the mock issuer, dev/auth-header.sh (AUTH_MODE=proxy-header
#               sends X-Auth-Request-Email to an orchestrator without the edge). Another user than the thread's
#               owner is a 404, exactly as in the web (an administrator reads every thread, and exports it too).
#
# Exit status: 0 when the file was written and checks out; 1 otherwise (the server's problem is printed
# when it refused). Needs: curl, jq.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  echo "usage: $0 THREAD_ID [OUT_FILE | -]" >&2
  exit 2
fi
id=$1
out=${2:-thread-$id.json}
case $id in
  *[!0-9a-fA-F-]* | '') echo "FAIL '$id' is not a thread id (a UUID)" >&2; exit 2 ;;
esac

# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

code=$(curl -sS --max-time 60 -o "$tmp/export.json" -w '%{http_code}' \
  -H "$id_header" "$base/api/threads/$id/export" || true)
if [ "$code" != 200 ]; then
  echo "FAIL GET /api/threads/$id/export answered HTTP ${code:-none}: $(head -c 400 "$tmp/export.json" 2>/dev/null)" >&2
  exit 1
fi

# The document is what it says it is: our format, a version this script knows, and a log without a gap.
summary=$(jq -er '
  select(.format == "another-agentic-system/thread-export" and .version == 1)
  | select([.events[].seq] == [range(1; (.events | length) + 1)])
  | "\(.events | length) events, state \(.thread.state)"
    + (if .job.attempt then ", attempt \(.job.attempt)" else "" end)
    + (if .eventsTruncated then ", TRUNCATED: the log is longer than the export reads" else "" end)
' "$tmp/export.json") || { echo "FAIL the answer is not a version 1 thread export with a gapless log" >&2; exit 1; }

if [ "$out" = - ]; then
  cat "$tmp/export.json"
else
  cp -- "$tmp/export.json" "$out"
  echo "wrote $out" >&2
fi
echo "thread $id: $summary" >&2
