#!/usr/bin/env sh
# System-level test of the usage ledger of the people (ADR 0065, docs/api/chat-api.yaml `getMyUsage`): what a person's agents spent in a
# UTC month is counted per task, priced with the table of dev/orchestrator.yaml when it is read, and outlives the threads that made it.
#
#   dev/usage-ledger-e2e.sh
#
# Start the `app` profile first (the mock issuer is built with the users below, so build it):
#
#   docker compose --profile app up -d --build --wait
#
# The agent is `mock-usage` (dev/agents.yaml, dev/wiremock/usage, ours), the same as dev/usage-e2e.sh uses: every run reports three calls
# (`c1`, `c2` under a sub-agent step, `c1` again as a replay, `c3`) and, read with `GetTask`, the totals of the task: glm-5.3 3600 in (1000
# of them read from a cache), 200 out (20 of them reasoning), glm-5.3-mini 600 in, 40 out, and a measured kubernetes `sandbox` of one shape
# (250 millicores, 512 MiB, 12 commands, 81 234 ms). The ledger counts a task by its TOTALS (a call counts only while the task has none,
# and a replayed call once), so each run is one task of those numbers, whatever the calls said.
#
# The people. `dev@example.com` has the usage of every other scenario that runs an agent which reports it (the coder does), so a month of
# `dev` cannot be asserted to the token. This script signs in as two people of dev/mock-oidc/users.json that no other scenario uses:
# `usage-ledger@example.com`, who runs the two threads, and `usage-bystander@example.com`, who runs none. The ledger is durable and the
# stack is not recreated between runs, so the script counts the ledger user's month first and, when it is not empty (a second run on the
# same database), SKIPS (exit 77, as dev/ci-e2e.sh does): `docker compose --profile app down -v` and up again, or LEDGER_EMAIL=<a user of
# users.json with no usage>, runs it again.
#
# **A child round is counted once** (ADR 0072 decision 4, ADR 0065's amendment of 2026-11-18). A third person, `usage-child@example.com`, used
# by no other scenario, starts one thread of `mock-sessions` (dev/wiremock/sessions, the agent of dev/child-sessions-e2e.sh): the parent opens a
# session of its helper and ENDS while the helper still works; the round, a task of its own (`<task>-s1`) in a child thread, reports its tokens
# after that. The parent's totals already hold them, so the child's own report is logged and not counted. The month is read before and after
# and only the DIFFERENCE is asserted, so this part runs on every run, the second one on a database included. By hand, from the mock:
#   the parent's totals (get-task-parent.json)  glm-5.3 1000 in, 100 out, 1100 (its own call p1); glm-5.3-mini 400 in, 50 out, 450 (the helper's h1)
#   the child's totals  (get-task-round.json)   glm-5.3-mini 400 in, 50 out, 450: the SAME tokens
#   so the month grows by   tasks 1 (one row, the parent's task)
#     glm-5.3       1000/100/1100, price 1000 * 0.40 + 100 * 1.60 = 400 + 160 = 560 millionths (a whole number: no rounding in the difference)
#     glm-5.3-mini  400/50/450, no price (counted twice it would be 800/100/900, and tasks 2)
#     sandbox       nothing (the mock measures none); total.amount + 560 millionths
#
# The prices are those of `usage` in dev/orchestrator.yaml (EUR; glm-5.3 input 0.40, cached input 0.10, output 1.60 per million tokens;
# glm-5.3-mini has NO price on purpose; the sandbox 0.04 per core-hour and 0.005 per GiB-hour). The expected amounts below are computed by
# hand from them and written as constants; change a price there and change them here.
#
#   two tasks, so per model the sum of the two TOTALS:
#     glm-5.3       input 7200 (cached 2000), output 400 (reasoning 40), total 7600
#       (7200 - 2000) * 0.40 + 2000 * 0.10 + 400 * 1.60          per million tokens
#       = 2 080 000 000 + 200 000 000 + 640 000 000 millionths / 1 000 000 = 2920 millionths = "0.002920"
#       (counted from the calls instead, with the replayed c1 twice, input would be 4800 a task and the price 3 millionths higher a task)
#     glm-5.3-mini  input 1200, output 80, total 1280, no price: "price": null, named in total.unpriced.models
#     sandbox       one shape, 250 millicores and 512 MiB: 24 commands, 162 468 ms
#       162468 ms / 3 600 000 ms per hour * (0.25 core * 0.04 + 0.5 GiB * 0.005)
#       = 0.045130 h * 0.0125 = 0.000564125 = 564.125 millionths, rounded half up once: 564 = "0.000564"
#     total.amount  the sum of the lines that are priced (the rounded lines): 2920 + 564 = 3484 millionths = "0.003484"
#   and "complete": false, because glm-5.3-mini has no price.
#
# It prints one ok or FAIL line per check and exits 1 if any failed (77 when it skips and nothing failed, 2 when the stack is not the one
# it needs):
#   * first, the child round: the parent of `mock-sessions` ends `done`, so does its child thread, whose log holds the round's own totals (so
#     the helper's report did arrive); the month of `usage-child@example.com` grew by the difference above and by nothing else;
#   * the ledger user's month is empty to begin with; two threads of `mock-usage` end `done`;
#   * GET /api/me/usage, with no month and with the current UTC month named, answers 200 as JSON with `Cache-Control: no-store`, `tasks: 2`,
#     the two models and the sandbox line above, currency EUR, a `prices` id of `sha256:` and sixteen hex digits, no unmeasured task, and
#     `total` `0.003484`, `complete: false`, naming glm-5.3-mini;
#   * another person reads an empty month that is complete (tasks 0, nothing listed, 0.000000) priced with the same table, and `?owner=` and
#     `?user=` are 400 for them (the route has no owner: nobody asks for another person's usage);
#   * `?month=2026-13`, a month that is not `YYYY-MM` and a month twice are 400; no token is 401;
#   * one of the two threads is deleted (204, and a 404 after), the purge of its files is awaited, and the answer is the same as before:
#     `tasks: 2`, the same amounts (the ledger outlives the thread);
#   * the answer holds nothing of the threads: no word of the agent's replies, of the person's message or of the orchestrator's own title,
#     and no thread id.
# Exit status 0 when every check passed.
#
# The month. The script reads the current UTC month (`date -u +%Y-%m`) before the first thread and again at the end. A task is counted in
# the month of the latest event that changed it (ADR 0065, build item 2), so a run that crosses the end of a UTC month lands its totals in the
# later month and the checks above cannot hold: the script then fails with a message that says so. It is rare; run it again.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL          http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   LEDGER_EMAIL      usage-ledger@example.com, who runs the two threads (a user of dev/mock-oidc/users.json)
#   CHILD_EMAIL       usage-child@example.com, who runs the thread with the child round
#   BYSTANDER_EMAIL   usage-bystander@example.com, who runs none
#   ORCH_SERVICE      orchestrator, the compose service whose metrics tell the purge is over (needs docker compose; without it the script waits 5 s)
#   TIMEOUT           120    seconds to wait for a thread to stop
#
# It does not touch the request journals of the mocks, so it can run beside the other scenarios. Needs curl and jq (and python3, or
# /proc, uuidgen for a v7 UUID). CI runs it in .github/workflows/coder-e2e.yml (dev/e2e-all.sh); the ledger and the route are also proven by the
# Rust tests (orchestrator/crates/store-postgres, orchestrator/crates/api/tests/usage.rs).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
ledger_email=${LEDGER_EMAIL:-usage-ledger@example.com}
bystander_email=${BYSTANDER_EMAIL:-usage-bystander@example.com}
child_email=${CHILD_EMAIL:-usage-child@example.com}
service=${ORCH_SERVICE:-orchestrator}
timeout=${TIMEOUT:-120}

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
if ! ledger_header=$(sh "$here/auth-header.sh" "$ledger_email") || ! bystander_header=$(sh "$here/auth-header.sh" "$bystander_email") ||
  ! child_header=$(sh "$here/auth-header.sh" "$child_email"); then
  echo "no token for $ledger_email, $bystander_email or $child_email: the mock issuer must be built with them (dev/mock-oidc/users.json): docker compose --profile app up -d --build mock-oidc" >&2
  exit 2
fi

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

uuid() { python3 -c 'import os,time;m=time.time_ns()//1000000;print("%08x-%04x-7%03x-%04x-%012x"%(m>>16,m&65535,int.from_bytes(os.urandom(2),"big")&4095,32768|int.from_bytes(os.urandom(2),"big")&16383,int.from_bytes(os.urandom(6),"big")))' 2>/dev/null || { echo 'uuid: python3 is missing, so this is a v4 id and recent will not be creation order' >&2; cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }; }

# fetch HEADER PATH: GET PATH as the person HEADER names (an empty HEADER sends no token). Prints the HTTP status; the body is $tmp/body,
# the response headers $tmp/headers.
fetch() {
  if [ -n "$1" ]; then
    curl -sS --max-time 60 -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' -H "$1" "$base$2" || echo 000
  else
    curl -sS --max-time 60 -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' "$base$2" || echo 000
  fi
}

# status HEADER METHOD PATH: the HTTP status of a request with no body wanted.
status() {
  curl -sS --max-time 60 -o /dev/null -w '%{http_code}' -X "$2" -H "$1" "$base$3" || echo 000
}

# run_agui, wait_state and settle act as the person `who` names.
# run_agui AGENT THREAD TEXT: one run (a message) on a new thread, to its end. Prints the HTTP status.
run_agui() {
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$who" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

# wait_state THREAD: waits (up to TIMEOUT seconds) until the thread stops, and prints the state it ended in (or the last one it saw).
wait_state() {
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(curl -sS --max-time 30 -H "$who" "$base/api/threads/$1" 2>/dev/null | jq -r '.state // empty' 2>/dev/null || true)
    case $_state in done | blocked | failed | cancelled) echo "$_state"; return 0 ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then echo "${_state:-unknown}"; return 0; fi
    sleep 2
  done
}

# settle THREAD: the stack's title and description models write after the job (events of their own): wait for both, up to 15 s, so that
# a delete does not race a late reply (dev/usage-e2e.sh does the same). A stack without them goes on.
settle() {
  _deadline=$(( $(date +%s) + 15 ))
  while :; do
    _n=$(curl -sS --max-time 30 -H "$who" "$base/api/threads/$1/export" 2>/dev/null |
      jq -r '[.events[] | select(.kind == "thread_titled" or .kind == "thread_described") | .kind] | unique | length' 2>/dev/null || echo 0)
    if [ "$_n" = 2 ] || [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 1
  done
}

purges_pending() { # the orchestrator's thread_purges_pending, or `none` (read from inside the stack, as dev/delete-e2e.sh does)
  docker compose exec -T edge wget -qO- "http://$service:8080/metrics" 2>/dev/null |
    awk '$1 == "thread_purges_pending" { print $2; found = 1 } END { if (!found) print "none" }'
}

# --- the stack -------------------------------------------------------------------------------------------------------------
agents=$(curl -sS --max-time 30 -H "$ledger_header" "$base/api/agents" 2>"$tmp/err" | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
case " $agents " in
  *" mock-usage "*) ok "GET /api/agents lists mock-usage" ;;
  *) echo "the agent 'mock-usage' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac
case " $agents " in
  *" mock-sessions "*) ok "GET /api/agents lists mock-sessions" ;;
  *) echo "the agent 'mock-sessions' is not listed by GET /api/agents (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
esac

month=$(date -u +%Y-%m)
code=$(fetch "$ledger_header" /api/me/usage)
if [ "$code" != 200 ]; then
  echo "GET /api/me/usage answered $code (not 200): is the orchestrator a build with the usage ledger, ADR 0065? $(head -c 200 "$tmp/body")" >&2
  exit 2
fi
if [ "$(jq -r '.currency' "$tmp/body")" != EUR ]; then
  echo "the orchestrator has no price table (GET /api/me/usage says currency '$(jq -r '.currency' "$tmp/body")'): the \`usage\` block of dev/orchestrator.yaml is read at start, so restart it: docker compose --profile app up -d --force-recreate orchestrator" >&2
  exit 2
fi
# --- a child round is counted once -------------------------------------------------------------------------------------------
# A difference of the month, so it is before the skip below and runs on a database an earlier run has used.
echo "== a child round that outlives its parent's task is counted once in the month of $child_email"
who=$child_header

# month_of FILE: the child user's month in FILE; stops the script when the answer is not a month.
month_of() {
  _code=$(fetch "$child_header" "/api/me/usage?month=$month")
  if [ "$_code" != 200 ]; then bad "GET /api/me/usage as $child_email answered $_code"; exit 1; fi
  cp "$tmp/body" "$1"
}

# export_child THREAD FILE: the thread's log in FILE
export_child() { curl -sS --max-time 30 -H "$child_header" "$base/api/threads/$1/export" >"$2" 2>/dev/null || echo '{"events":[]}' >"$2"; }

month_of "$tmp/child.before.json"
parent=$(uuid)
code=$(run_agui mock-sessions "$parent" "plan the migration ($(uuid))")
expect "POST /agui/agents/mock-sessions answers 200 ($parent)" "$code" "200"
expect "the parent thread ends done" "$(wait_state "$parent")" "done"
settle "$parent"
# The helper works on after the parent ended: the month is read only once its round has closed the session and ended the child, so that a
# second count of it, were there one, is in the ledger by then.
_deadline=$(( $(date +%s) + timeout ))
while :; do
  export_child "$parent" "$tmp/parent.export.json"
  _closed=$(jq -r '[.events[] | select(.kind == "child_session_closed")] | length' "$tmp/parent.export.json" 2>/dev/null || echo 0)
  if [ "$_closed" -ge 1 ] || [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
  sleep 1
done
expect "the round outlived the parent's task: the parent's own completed comes before the round's report and the close" \
  "$(jq -r '[.events[] | select(.kind == "child_session_reported" or .kind == "child_session_closed" or (.kind == "agent_status" and .data.status == "completed"))
     | if .kind == "agent_status" then "completed" else .kind end] | join(",")' "$tmp/parent.export.json")" \
  "completed,child_session_reported,child_session_closed"
child=$(jq -r '[.events[] | select(.kind == "child_session_opened") | .data.thread] | first // ""' "$tmp/parent.export.json")
if [ -n "$child" ]; then
  expect "the child thread ends done" "$(wait_state "$child")" "done"
  export_child "$child" "$tmp/child.export.json"
  expect "the child's report did arrive: its log holds the round's own totals, the helper's tokens again" \
    "$(jq -r '[.events[] | select(.kind == "model_usage_total") | .data.totals[] | "\(.model):\(.inputTokens)/\(.outputTokens)/\(.totalTokens)"] | join(" ")' "$tmp/child.export.json")" \
    "glm-5.3-mini:400/50/450"
else
  bad "the parent names no child thread"
fi
month_of "$tmp/child.after.json"

# delta EXPRESSION: EXPRESSION of the month after less EXPRESSION of the month before.
delta() { jq -n --slurpfile a "$tmp/child.after.json" --slurpfile b "$tmp/child.before.json" "(\$a[0] | $1) - (\$b[0] | $1)"; }
# model_delta MODEL: "input/output/total" the month grew by for MODEL.
model_delta() {
  _out=
  for _f in inputTokens outputTokens totalTokens; do
    _out="$_out${_out:+/}$(delta "([.models[] | select(.model == \"$1\") | .$_f] | add // 0)")"
  done
  echo "$_out"
}
# millionths MODEL-OR-total: how many millionths of the currency its price grew by.
millionths() { delta "([$1 | select(. != null) | tonumber * 1000000 | round] | add // 0)"; }

expect "child round: the month grew by one task, the parent's: one row, none for the child's" "$(delta .tasks)" "1"
expect "child round: glm-5.3 grew by the parent's own call, 1000/100/1100" "$(model_delta glm-5.3)" "1000/100/1100"
expect "child round: glm-5.3-mini grew by the helper's tokens ONCE, 400/50/450 (twice would be 800/100/900)" \
  "$(model_delta glm-5.3-mini)" "400/50/450"
expect "child round: no other model grew, and no sandbox line" \
  "$(delta '([.models[] | select(.model != "glm-5.3" and .model != "glm-5.3-mini") | .totalTokens] | add // 0)') $(delta '([.sandbox[] | .commands] | add // 0)')" "0 0"
expect "child round: glm-5.3 is priced 560 millionths more, and total.amount 560 millionths more" \
  "$(millionths '.models[] | select(.model == "glm-5.3") | .price') $(millionths .total.amount)" "560 560"
echo "     the difference: tasks $(delta .tasks), glm-5.3 $(model_delta glm-5.3), glm-5.3-mini $(model_delta glm-5.3-mini)"
who=$ledger_header

# --- the ledger user -------------------------------------------------------------------------------------------------------
tasks=$(fetch "$ledger_header" /api/me/usage >/dev/null; jq -r '.tasks' "$tmp/body")
if [ "$tasks" != 0 ]; then
  # a failure of the child round above is not hidden by the skip
  if [ "$fail" -ne 0 ]; then echo "usage ledger e2e FAILED"; exit 1; fi
  echo "SKIP  $ledger_email already has $tasks task(s) in $month: an earlier run on this database"
  echo "      the scenario counts exactly its own two tasks; recreate the stack (docker compose --profile app down -v, then up) or set LEDGER_EMAIL"
  exit 77
fi
ok "the month of $ledger_email is empty to begin with ($month)"

# --- two threads of an agent that reports its usage ------------------------------------------------------------------------
echo "== two threads of mock-usage, each run to its end"
nonce=$(uuid)
thread_a=$(uuid)
thread_b=$(uuid)
for t in "$thread_a" "$thread_b"; do
  code=$(run_agui mock-usage "$t" "summarize the notes ($nonce)")
  expect "POST /agui/agents/mock-usage answers 200 ($t)" "$code" "200"
  expect "the thread $t ends done" "$(wait_state "$t")" "done"
  settle "$t"
done

# --- the expected answer ---------------------------------------------------------------------------------------------------
# Amounts are strings with six decimals; `prices` is masked (its value is checked on its own). See the arithmetic in the header.
expected='{
  "month": "MONTH",
  "currency": "EUR",
  "prices": "PRICES",
  "tasks": 2,
  "models": [
    {"provider": "openai", "model": "glm-5.3", "inputTokens": 7200, "outputTokens": 400, "totalTokens": 7600,
     "reasoningTokens": 40, "cachedInputTokens": 2000, "price": "0.002920"},
    {"provider": "openai", "model": "glm-5.3-mini", "inputTokens": 1200, "outputTokens": 80, "totalTokens": 1280, "price": null}
  ],
  "sandbox": [
    {"environment": "kubernetes", "cpuMillis": 250, "memoryBytes": 536870912, "commands": 24, "milliseconds": 162468, "price": "0.000564"}
  ],
  "unmeasured": [],
  "total": {
    "amount": "0.003484", "currency": "EUR", "complete": false,
    "unpriced": {"models": [{"provider": "openai", "model": "glm-5.3-mini"}], "sandbox": 0, "unmeasured": 0}
  }
}'

# check_month LABEL PATH EXPECTED_MONTH: GET PATH as the ledger user and compare the whole answer with the expected one.
check_month() {
  _code=$(fetch "$ledger_header" "$2")
  expect "$1: 200" "$_code" "200"
  expect "$1: JSON, and not cached" \
    "$(tr -d '\r' <"$tmp/headers" | awk -F': *' 'tolower($1) == "content-type" { ct = ($2 ~ /^application\/json/) ? "json" : $2 } tolower($1) == "cache-control" { cc = $2 } END { print ct " " cc }')" \
    "json no-store"
  expect "$1: the prices id is sha256: and sixteen hex digits" \
    "$(jq -r '.prices | test("^sha256:[0-9a-f]{16}$")' "$tmp/body")" "true"
  jq -S '.prices = "PRICES"' "$tmp/body" >"$tmp/got.json" 2>/dev/null || echo 'null' >"$tmp/got.json"
  printf '%s' "$expected" | jq -S --arg m "$3" '.month = $m' >"$tmp/want.json"
  if cmp -s "$tmp/got.json" "$tmp/want.json"; then
    ok "$1: the whole answer is the expected one (per model the sum of the two totals, priced, the sandbox priced, the total incomplete)"
  else
    bad "$1: the answer is not the expected one"
    diff "$tmp/want.json" "$tmp/got.json" | sed 's/^/     /' || true
  fi
  expect "$1: tasks" "$(jq -r '.tasks' "$tmp/body")" "2"
  expect "$1: complete is false and names glm-5.3-mini" \
    "$(jq -r '[.total.complete, (.total.unpriced.models | map(.model) | join(","))] | join(" ")' "$tmp/body")" "false glm-5.3-mini"
  expect "$1: glm-5.3-mini has price null, not a price of zero" \
    "$(jq -r '[.models[] | select(.model == "glm-5.3-mini") | .price] | map(tostring) | join(",")' "$tmp/body")" "null"
  expect "$1: total.amount is the sum of the priced lines" \
    "$(jq -r '[.models[], .sandbox[] | .price | select(. != null) | tonumber * 1000000 | round] | add | tostring' "$tmp/body")" "3484"
  cp "$tmp/body" "$tmp/$4"
}

echo "== GET /api/me/usage as $ledger_email"
check_month "default month" "/api/me/usage" "$month" before-default.json
check_month "month=$month" "/api/me/usage?month=$month" "$month" before-explicit.json
expect "the default and the named month are one answer" \
  "$(jq -cS . "$tmp/before-default.json")" "$(jq -cS . "$tmp/before-explicit.json")"
prices_id=$(jq -r '.prices' "$tmp/before-default.json")

# --- another person --------------------------------------------------------------------------------------------------------
echo "== another person has nothing, and cannot ask for the first one's"
code=$(fetch "$bystander_header" /api/me/usage)
expect "the bystander: 200" "$code" "200"
expect "the bystander: an empty month that is complete, in the same currency and priced with the same table" \
  "$(jq -cS . "$tmp/body")" \
  "$(jq -cnS --arg m "$month" --arg p "$prices_id" '{month: $m, currency: "EUR", prices: $p, tasks: 0, models: [], sandbox: [], unmeasured: [],
     total: {amount: "0.000000", currency: "EUR", complete: true, unpriced: {models: [], sandbox: 0, unmeasured: 0}}}')"
for q in "owner=$ledger_email" "user=$ledger_email" "owner=dev" "user=dev" "month=$month&owner=$ledger_email" "email=$ledger_email"; do
  expect "the bystander: ?$q is 400" "$(fetch "$bystander_header" "/api/me/usage?$q")" "400"
done
expect "400 is a problem document" \
  "$(tr -d '\r' <"$tmp/headers" | awk -F': *' 'tolower($1) == "content-type" { print ($2 ~ /^application\/problem\+json/) ? "problem" : $2 }')" "problem"
expect "the ledger user: ?owner= is 400 too (no owner parameter for anybody)" "$(fetch "$ledger_header" "/api/me/usage?owner=$ledger_email")" "400"

# --- what is not a month, and no token -------------------------------------------------------------------------------------
echo "== a month that is not one, and no token"
for q in "month=2026-13" "month=2026-00" "month=garbage" "month=2026-1" "month=" "month=$month&month=$month"; do
  expect "?$q is 400" "$(fetch "$ledger_header" "/api/me/usage?$q")" "400"
done
expect "no token is 401" "$(fetch "" /api/me/usage)" "401"

# --- the ledger outlives the thread ----------------------------------------------------------------------------------------
echo "== one thread is deleted; the ledger keeps what it made"
expect "the thread $thread_a still reads before" "$(status "$ledger_header" GET "/api/threads/$thread_a")" "200"
expect "DELETE of the thread is 204" "$(status "$ledger_header" DELETE "/api/threads/$thread_a")" "204"
expect "the thread is a 404" "$(status "$ledger_header" GET "/api/threads/$thread_a")" "404"
_deadline=$(( $(date +%s) + 30 ))
_pending=$(purges_pending)
if [ "$_pending" = none ]; then
  sleep 5
  ok "the purge was not asked about (no docker compose here): waited 5 s"
else
  while [ "$_pending" != 0 ] && [ "$(date +%s)" -lt "$_deadline" ]; do sleep 1; _pending=$(purges_pending); done
  expect "nothing is left to purge (thread_purges_pending)" "$_pending" "0"
fi
expect "the other thread is untouched" "$(status "$ledger_header" GET "/api/threads/$thread_b")" "200"
check_month "after the delete, default month" "/api/me/usage" "$month" after-default.json
check_month "after the delete, month=$month" "/api/me/usage?month=$month" "$month" after-explicit.json
expect "the answer after the delete is the answer before it, byte for byte once sorted" \
  "$(jq -cS . "$tmp/after-default.json")" "$(jq -cS . "$tmp/before-default.json")"

# --- no thread content -----------------------------------------------------------------------------------------------------
echo "== the answer holds nothing of the threads"
leaks=0
for word in "Summary: three calls" "Reading the notes" "summarize the notes" "$nonce" "Mock thread" "$thread_a" "$thread_b" "task-"; do
  if grep -qiF -e "$word" "$tmp/before-default.json" "$tmp/after-default.json"; then
    bad "the answer holds '$word'"
    leaks=1
  fi
done
[ "$leaks" -eq 1 ] || ok "no word of the replies, of the message, of the title, no thread id and no task id in the answers"

# --- the month did not turn ------------------------------------------------------------------------------------------------
month_end=$(date -u +%Y-%m)
if [ "$month_end" != "$month" ]; then
  bad "the run crossed the end of the UTC month ($month, then $month_end): the tasks that finished after midnight are counted in $month_end (ADR 0065, build item 2) and the checks above compare a month that moved; run it again"
fi

if [ "$fail" -eq 0 ]; then echo "usage ledger e2e passed"; else echo "usage ledger e2e FAILED"; exit 1; fi
