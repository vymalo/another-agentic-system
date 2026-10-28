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
# It prints one ok or FAIL line per check and exits 1 if any failed (77 when it skips, 2 when the stack is not the one it needs):
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
#   BYSTANDER_EMAIL   usage-bystander@example.com, who runs none
#   ORCH_SERVICE      orchestrator, the compose service whose metrics tell the purge is over (needs docker compose; without it the script waits 5 s)
#   TIMEOUT           120    seconds to wait for a thread to stop
#
# It does not touch the request journals of the mocks, so it can run beside the other scenarios. Needs curl and jq (and /proc or uuidgen
# for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml (dev/e2e-all.sh); the ledger and the route are also proven by the
# Rust tests (orchestrator/crates/store-postgres, orchestrator/crates/api/tests/usage.rs).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
ledger_email=${LEDGER_EMAIL:-usage-ledger@example.com}
bystander_email=${BYSTANDER_EMAIL:-usage-bystander@example.com}
service=${ORCH_SERVICE:-orchestrator}
timeout=${TIMEOUT:-120}

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
if ! ledger_header=$(sh "$here/auth-header.sh" "$ledger_email") || ! bystander_header=$(sh "$here/auth-header.sh" "$bystander_email"); then
  echo "no token for $ledger_email or $bystander_email: the mock issuer must be built with them (dev/mock-oidc/users.json): docker compose --profile app up -d --build mock-oidc" >&2
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

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

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

# run_agui AGENT THREAD TEXT: one run (a message) on a new thread, to its end. Prints the HTTP status.
run_agui() {
  _input=$(jq -n --arg thread "$2" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$3" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST "$base/agui/agents/$1" -H "$ledger_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

# wait_state THREAD: waits (up to TIMEOUT seconds) until the thread stops, and prints the state it ended in (or the last one it saw).
wait_state() {
  _deadline=$(( $(date +%s) + timeout ))
  _state=
  while :; do
    _state=$(curl -sS --max-time 30 -H "$ledger_header" "$base/api/threads/$1" 2>/dev/null | jq -r '.state // empty' 2>/dev/null || true)
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
    _n=$(curl -sS --max-time 30 -H "$ledger_header" "$base/api/threads/$1/export" 2>/dev/null |
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
tasks=$(jq -r '.tasks' "$tmp/body")
if [ "$tasks" != 0 ]; then
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
