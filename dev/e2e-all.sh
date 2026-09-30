#!/usr/bin/env sh
# Runs every scenario script of the local stack against a running `app` profile, one after the other, and
# prints a summary. This is the "does my stack work" command.
#
#   docker compose --profile app up -d --build --wait     # once (the first build takes several minutes)
#   dev/e2e-all.sh                                        # every scenario
#   dev/e2e-all.sh verify mcp                             # only these
#   VERBOSE=1 dev/e2e-all.sh                              # stream every script's own output
#
# The scenarios, each one script of this directory (the header of a script says exactly what it asserts):
#
#   coder             chat -> coder -> branch -> mock-ci -> green -> pull request   coder-e2e.sh
#   coder-no-opencode the same, the check command makes the change (no OpenCode)    NO_OPENCODE=1 coder-e2e.sh
#   verify            red once -> rework -> green; red always -> failed; the gate    verify-e2e.sh
#                     cannot be weakened by a run
#   verifier          the verifier finds fault -> rework -> the verifier passes      verifier-e2e.sh
#   mcp               an MCP client starts a job and follows it, with progress       mcp-e2e.sh
#   ci                a signed CI report sends the agent back, then ends the job     ci-e2e.sh
#
# `ci` passes once per database (a commit belongs to the first job that pushed it): on a second run it is
# SKIPPED, not failed, and the summary says how to reset (`docker compose --profile app down -v`).
# The split roles (dev/split-e2e.sh) need another shape of the stack and are not part of this list.
#
# Each script's output goes to a file, and only the tail of a failing one is printed; the file is kept in
# $LOG_DIR (default: a fresh directory under ${TMPDIR:-/tmp}) and named in the summary. Environment that the
# scripts read (BASE_URL, EDGE_PORT, AUTH_EMAIL, TIMEOUT, ...) is passed through unchanged.
#
# Exit status: 0 when no scenario failed (a skip is not a failure), 1 when one did, 2 on a usage error or
# when the stack is not there. Needs curl, jq, git and openssl (and docker compose, only to print logs).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
export BASE_URL="$base"

all="coder coder-no-opencode verify verifier mcp ci"
# shellcheck disable=SC2086 # the list is words on purpose
[ "$#" -gt 0 ] || set -- $all
for s in "$@"; do
  case " $all " in
    *" $s "*) ;;
    *) echo "unknown scenario '$s'; choose from: $all" >&2; exit 2 ;;
  esac
done

for tool in curl jq git openssl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

# --- is the stack there? ---------------------------------------------------------------------------------
if ! curl -fsS --max-time 10 "$base/readyz" >/dev/null 2>&1; then
  cat >&2 <<EOF
Nothing answers at $base/readyz. Start the stack first (the first build takes several minutes, and the
coder image is about 2.9 GB):

  docker compose --profile app up -d --build --wait

or point BASE_URL (or EDGE_PORT) at the edge you started. See dev/README.md, "Test it locally".
EOF
  exit 2
fi
agents=$(curl -fsS --max-time 30 -H "X-Auth-Request-Email: ${AUTH_EMAIL:-dev@example.com}" "$base/api/agents" 2>/dev/null | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
echo "stack: $base, agents: ${agents:-none}"
for s in "$@"; do
  case $s in
    coder | coder-no-opencode)
      case " $agents " in
        *" coder "*) ;;
        *) echo "scenario $s needs the agent 'coder', which GET /api/agents does not list: is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
      esac ;;
  esac
done

log_dir=${LOG_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/e2e-all.XXXXXX")}
mkdir -p "$log_dir"
summary=$(mktemp)
trap 'rm -f "$summary"' EXIT

passed=0
failed=0
skipped=0

# run NAME COMMAND...: runs one scenario, records its result in $summary.
run() {
  name=$1
  shift
  log=$log_dir/$name.log
  printf '== %s\n' "$name"
  started=$(date +%s)
  if [ "${VERBOSE:-}" = 1 ]; then
    # `tee` would hide the script's status, so the status goes through a file.
    { rc=0; "$@" 2>&1 || rc=$?; echo "$rc" >"$log.rc"; } | tee "$log"
    rc=$(cat "$log.rc")
  else
    rc=0
    "$@" >"$log" 2>&1 || rc=$?
  fi
  took=$(($(date +%s) - started))
  case $rc in
    0)
      passed=$((passed + 1))
      printf 'ok    %-18s %4ss\n' "$name" "$took" | tee -a "$summary" ;;
    77)
      skipped=$((skipped + 1))
      printf 'SKIP  %-18s %4ss  (%s)\n' "$name" "$took" "$(sed -n 's/^SKIP  *//p' "$log" | head -n 1 | cut -c1-100)" | tee -a "$summary"
      sed -n 's/^      //p' "$log" | head -n 2 | sed 's/^/      /' ;;
    *)
      failed=$((failed + 1))
      printf 'FAIL  %-18s %4ss  log: %s\n' "$name" "$took" "$log" | tee -a "$summary"
      if [ "${VERBOSE:-}" != 1 ]; then
        echo "  --- the last lines of its output"
        tail -n 25 "$log" | sed 's/^/  | /'
      fi ;;
  esac
}

for s in "$@"; do
  case $s in
    coder) run coder sh "$here/coder-e2e.sh" ;;
    coder-no-opencode) run coder-no-opencode env NO_OPENCODE=1 sh "$here/coder-e2e.sh" ;;
    verify) run verify sh "$here/verify-e2e.sh" ;;
    verifier) run verifier sh "$here/verifier-e2e.sh" ;;
    mcp) run mcp sh "$here/mcp-e2e.sh" ;;
    ci) run ci sh "$here/ci-e2e.sh" ;;
  esac
done

echo
echo "== summary ($base)"
cat "$summary"
echo "$passed passed, $failed failed, $skipped skipped; logs in $log_dir"
if [ "$skipped" -gt 0 ]; then
  echo "a skipped scenario passed in an earlier run of this database; to run it again from scratch:"
  echo "  docker compose --profile app down -v && docker compose --profile app up -d --build --wait"
fi
if [ "$failed" -gt 0 ]; then
  echo "the logs of the stack: docker compose --profile app logs --no-color --tail 100 orchestrator mock-ci coder"
  exit 1
fi
exit 0
