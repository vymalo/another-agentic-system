#!/usr/bin/env sh
# The proof (sys#334 S9, ANR-662, ANR-667 and ANR-669), with the real Authorino over gRPC and WireMock as the model
# gateway: the agent presents the service-account token of its pod (TokenReview, audience `spike`) and a run id;
# Authorino asks the broker's run-lease route for the run's key and account with the injector's own token, reads the
# person's balance from the ledger's budget read, and adds `Authorization`, `x-account-id` and `x-budget-remaining`
# upstream. Four things are checked:
#   1. the gateway's journal holds exactly one `Authorization` and exactly one `x-account-id`, the LEASED values: a
#      client that sends its own copies of both does not get them through (replaced, never merged);
#   2. a caller whose bearer is not a service-account token Authorino accepts (none at all, or a token for another
#      audience) is refused before the gateway sees the request (journal count unchanged);
#   3. a run with no usable lease reaches no gateway: no run id, a refused lease (410/404), or a lease with no token
#      or no account id;
#   4. the balance: a funded person's call passes, a person at zero and an account the ledger does not know are
#      refused with 402 (`model_locked`), a balance the ledger could not answer is refused with 403 — and none of
#      them reaches the gateway, whose journal does not move.
# Negative controls make the checks able to fail: without the balance filter a zero-balance run reaches the gateway,
# without the authorizer the balance filter answers 503 rather than let a request through, and without both the
# client's own header arrives.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
ctx=kind-agent-router-spike
kc() { kubectl --context "$ctx" "$@"; }

svc=$(kc -n envoy-gateway-system get svc -l gateway.envoyproxy.io/owning-gateway-name=spike \
  -o jsonpath='{.items[0].metadata.name}')
[ -n "$svc" ] || { echo "spike.sh: no gateway service; run up.sh first" >&2; exit 1; }

kc -n envoy-gateway-system port-forward "svc/$svc" 18080:80 >/dev/null 2>&1 &
pf=$!
# WireMock's own admin API, straight to its Service: the journal is what the gateway actually received.
kc -n default port-forward svc/spike-gateway 18081:80 >/dev/null 2>&1 &
pf2=$!
body=$(mktemp)
trap 'kill $pf $pf2 2>/dev/null || true; rm -f "$body"' EXIT

request() { # request ARGS...
  curl -sS -o "$body" -w '%{http_code}' -X POST http://127.0.0.1:18080/v1/chat/completions \
    -H 'content-type: application/json' -H 'x-ai-eg-model: spike-model' "$@"
}

admin() { curl -sS http://127.0.0.1:18081/__admin/requests; }

# req_last RUN: WireMock's journal entry for the newest request that carried `X-Run-Id: RUN` ({} if none). The journal
# is newest-first (its `/__admin/requests` lists the most recent request at index 0), and a retried warmup request can
# be recorded out of order, so select by run id and take the first, never by the array's end (seen 2026-11-24).
req_last() {
  admin | jq -c --arg r "$1" '
    [ .requests[]
      | select((.request.headers | to_entries
                | map(select(.key | ascii_downcase == "x-run-id"))
                | (.[0].value // "")) == $r)
    ][0].request // {}'
}
# count_for RUN: how many requests for RUN the gateway journaled.
count_for() {
  admin | jq -r --arg r "$1" '
    [ .requests[]
      | select((.request.headers | to_entries
                | map(select(.key | ascii_downcase == "x-run-id"))
                | (.[0].value // "")) == $r)
    ] | length'
}
# count_without_run: requests that carried no X-Run-Id at all.
count_without_run() {
  admin | jq -r '
    [ .requests[]
      | select((.request.headers | to_entries
                | map(select(.key | ascii_downcase == "x-run-id"))
                | (.[0].value // "")) == "")
    ] | length'
}
# hval RUN HEADER: the value the gateway received under HEADER on RUN's request, or "".
hval() { req_last "$1" | jq -r --arg k "$2" \
  '(.headers // {}) | to_entries | map(select(.key | ascii_downcase == ($k | ascii_downcase))) | (.[0].value // "")'; }
# hcount RUN HEADER: how many headers named HEADER (any case) RUN's request carried.
hcount() { req_last "$1" | jq -r --arg k "$2" \
  '(.headers // {}) | to_entries | map(select(.key | ascii_downcase == ($k | ascii_downcase))) | length'; }

fail() { echo "FAIL: $1" >&2; exit 1; }

# Wait for the port-forwards: any answer but none at all means the tunnel is up.
for _ in $(seq 1 30); do
  code=$(request -H 'X-Run-Id: warmup' -d '{"model":"spike-model","messages":[]}' || true)
  curl -fsS http://127.0.0.1:18081/__admin/health >/dev/null 2>&1 || code=""
  case "$code" in ""|000) sleep 1 ;; *) break ;; esac
done

marker=anr250-lease-marker-7f3c-run-1
account=acct-run-1
client_account=acct-client-DO-NOT-PASS

# The agent pod's identity: a real projected service-account token for the audience the AuthConfig trusts. Every request
# that should authenticate presents it; the injector replaces it upstream, exactly as it replaces the agent's own header.
agent_token=$(kc create token spike-agent --audience spike --duration 1h)
other_token=$(kc create token spike-agent --audience another-audience --duration 1h)
[ -n "$agent_token" ] && [ -n "$other_token" ] || { echo "spike.sh: could not mint the service-account tokens" >&2; exit 1; }
authn="Authorization: Bearer $agent_token"

echo "--- the agent's request: a client-supplied Authorization AND x-account-id are both replaced by the lease's"
before=$(count_for run-1)
code=$(request -H 'X-Run-Id: run-1' -H "$authn" -H "x-account-id: $client_account" \
  -d '{"model":"spike-model","messages":[{"role":"user","content":"hi"}]}')
after=$(count_for run-1)
echo "spike: HTTP $code, gateway requests for run-1 before=$before after=$after"
[ "$code" = 200 ] || { cat "$body"; echo; fail "expected 200"; }
[ "$after" -eq $((before + 1)) ] || fail "expected exactly one new request for run-1 at the gateway"

n=$(hcount run-1 authorization)
v=$(hval run-1 authorization)
na=$(hcount run-1 x-account-id)
a=$(hval run-1 x-account-id)
echo "gateway saw: Authorization occurrences=$n value=[$v]"
echo "gateway saw: x-account-id occurrences=$na value=[$a]"
[ "$n" -eq 1 ] || fail "expected exactly one Authorization at the gateway"
[ "$v" = "Bearer $marker" ] || fail "the leased Authorization did not reach the gateway (got [$v])"
[ "$na" -eq 1 ] || fail "expected exactly one x-account-id at the gateway"
[ "$a" = "$account" ] || fail "the leased account id did not reach the gateway (got [$a])"
case "$v" in *client-sent-DO-NOT-PASS*|*"$agent_token"*) fail "a client-sent Authorization survived" ;; esac
[ "$a" != "$client_account" ] || fail "the client's own x-account-id survived"
echo "PASS: the lease's key and account id reached the gateway; neither client-sent value did"

echo "--- the broker saw the lookup, with the injector's token, keyed by the run, and minted that account"
last=$(kc exec deploy/spike-broker -- python -c \
  'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:8080/__last").read().decode())')
echo "broker: $last"
printf '%s' "$last" | jq -e '.run == "run-1" and .injectorToken == true and .accountId == "'"$account"'"' >/dev/null \
  || fail "the broker was not asked for run-1 with the injector's token"
echo "PASS: Authorino asked the broker for the run's lease with its own token, and the lease named $account"

echo "--- the balance: the ledger was read for that lease's account, with the injector's own token, and never a debit"
ledger_last() {
  kc exec deploy/spike-ledger -- python -c \
    'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:8080/__last").read().decode())'
}
llast=$(ledger_last)
echo "ledger: $llast"
printf '%s' "$llast" | jq -e '.account == "'"$account"'" and .injectorToken == true and .path == "/budget/v1/remaining"' >/dev/null \
  || fail "the ledger was not read for $account with the injector's token"
echo "PASS: the injector read the ledger's budget for the lease's account, with its own secret"

echo "--- the balance stays in the injector: the model gateway never sees x-budget-remaining"
nb=$(hcount run-1 x-budget-remaining)
echo "gateway saw: x-budget-remaining occurrences=$nb"
[ "$nb" -eq 0 ] || fail "the person's balance reached the model gateway"
echo "PASS: the balance header was removed before the model gateway"

echo "--- negative control 1a: without the balance filter, a zero balance reaches the gateway (the 402 can fail)"
kc delete envoyextensionpolicy spike-balance --ignore-not-found >/dev/null
for _ in $(seq 1 15); do
  code=$(request -H 'X-Run-Id: zero-run' -H "$authn" -d '{"model":"spike-model","messages":[]}')
  [ "$code" = 200 ] && break
  sleep 2
done
echo "zero-run without the balance filter: HTTP $code, gateway requests for zero-run $(count_for zero-run)"
[ "$code" = 200 ] || fail "removing the balance filter did not let a zero-balance run through (the check cannot fail)"
[ "$(count_for zero-run)" -ge 1 ] || fail "the zero-balance run was allowed but the gateway journaled nothing"
echo "PASS: the balance filter is what refuses a spent balance; nothing else does"
echo "--- restoring the balance filter"
kc apply -f "$dir/manifests/32-budget-policy.yaml" >/dev/null
for _ in $(seq 1 30); do
  code=$(request -H 'X-Run-Id: zero-run' -H "$authn" -d '{"model":"spike-model","messages":[]}')
  [ "$code" = 402 ] && break
  sleep 2
done
[ "$code" = 402 ] || fail "the balance filter did not come back (got $code)"
echo "PASS: the 402 is back"

echo "--- negative control 1b: without the authorizer, the balance filter fails closed, never open"
kc delete securitypolicy spike-extauth --ignore-not-found >/dev/null
for _ in $(seq 1 15); do
  before=$(count_for run-noauth)
  code=$(request -H 'X-Run-Id: run-noauth' \
    -d '{"model":"spike-model","messages":[]}')
  after=$(count_for run-noauth)
  [ "$code" = 503 ] && break
  sleep 2
done
echo "spike: with no authorizer and no balance -> HTTP $code, gateway requests for run-noauth before=$before after=$after"
[ "$code" = 503 ] || fail "the balance filter let a request without a balance through (got $code)"
[ "$after" -eq "$before" ] || fail "a request without a balance reached the gateway"
echo "PASS: a missing balance is 503 budget_unavailable, never a pass"

echo "--- negative control 1: with both policies removed, the client's own header arrives"
kc delete envoyextensionpolicy spike-balance --ignore-not-found >/dev/null
# The policy is gone once a request that carries the client's marker reaches the gateway with it.
for _ in $(seq 1 15); do
  code=$(request -H 'X-Run-Id: run-nopolicy' -H 'Authorization: Bearer client-sent-NO-EXTAUTH' \
    -d '{"model":"spike-model","messages":[]}')
  if [ "$(hval run-nopolicy authorization)" = 'Bearer client-sent-NO-EXTAUTH' ]; then break; fi
  sleep 2
done
v=$(hval run-nopolicy authorization)
echo "Authorization at the gateway without the policy: [$v]"
[ "$v" = 'Bearer client-sent-NO-EXTAUTH' ] \
  || fail "the negative control did not reach the gateway (the check cannot fail)"
echo "PASS: the check can fail — the client's header is what arrives without the injector"

echo "--- restoring the policies"
kc apply -f "$dir/manifests/32-budget-policy.yaml" >/dev/null
kc apply -f "$dir/manifests/31-route-policy.yaml" >/dev/null
for _ in $(seq 1 30); do
  code=$(request -H 'X-Run-Id: run-1' -H "$authn" -H "x-account-id: $client_account" \
    -d '{"model":"spike-model","messages":[]}')
  if [ "$code" = 200 ] && [ "$(hval run-1 authorization)" = "Bearer $marker" ]; then break; fi
  sleep 2
done
[ "$(hval run-1 authorization)" = "Bearer $marker" ] || fail "the policy did not come back"
echo "PASS: the policies are back and the leased headers replace the client's again"

echo "--- negative control 2: without a run id, nothing reaches the gateway"
before=$(count_without_run)
code=$(request -H "$authn" -d '{"model":"spike-model","messages":[]}')
after=$(count_without_run)
echo "spike: without X-Run-Id: HTTP $code, gateway requests without a run before=$before after=$after"
case "$code" in
  401|403) ;;
  *) fail "expected 401 or 403, got $code" ;;
esac
[ "$after" -eq "$before" ] || fail "the refused request reached the gateway"
echo "PASS: the refused request reached no gateway"

# The identity half (ANR-667): the header is only trusted when the caller is an agent pod the injector accepts. A bearer
# the TokenReview does not accept — none, or one minted for another audience — is refused before the gateway.
refused_identity() { # refused_identity RUN LABEL [curl header args...]
  run=$1
  label=$2
  shift 2
  before=$(count_for "$run")
  code=$(request -H "X-Run-Id: $run" "$@" -d '{"model":"spike-model","messages":[]}')
  after=$(count_for "$run")
  echo "spike: $label -> HTTP $code, gateway requests for $run before=$before after=$after"
  case "$code" in
    401|403) ;;
    *) fail "$label: expected 401 or 403, got $code" ;;
  esac
  [ "$after" -eq "$before" ] || fail "$label was refused but still reached the gateway"
  echo "PASS: $label was refused at the authorizer ($code) and the gateway saw no request"
}

echo "--- the identity half: a caller with no accepted service-account token never reaches the gateway"
refused_identity run-no-bearer "no bearer at all"
refused_identity run-other-audience "a service-account token for another audience" \
  -H "Authorization: Bearer $other_token"

# A lease the broker refuses (410 lease_expired, 404 run_not_bound): either the authorizer stops the request, or it
# reaches the gateway with nothing injected. Say which happened, with the observable.
lease_control() { # lease_control RUN
  run=$1
  before=$(count_for "$run")
  code=$(request -H "X-Run-Id: $run" -H "$authn" -d '{"model":"spike-model","messages":[]}')
  after=$(count_for "$run")
  echo "spike: $run -> HTTP $code, gateway requests for $run before=$before after=$after"
  case "$code" in
    2*|4*|5*) ;;
    *) fail "$run: unexpected status $code" ;;
  esac
  if [ "$code" -ge 400 ]; then
    [ "$after" -eq "$before" ] || fail "$run was refused but still reached the gateway"
    echo "PASS: $run was refused at the authorizer ($code) and the gateway saw no request"
  else
    [ "$after" -eq $((before + 1)) ] || fail "$run: expected exactly one request at the gateway"
    v=$(hval "$run" authorization)
    a=$(hval "$run" x-account-id)
    echo "gateway saw: Authorization=[$v] x-account-id=[$a]"
    [ -z "$v" ] || fail "$run carried an Authorization to the gateway"
    [ -z "$a" ] || fail "$run carried an x-account-id to the gateway"
    echo "PASS: $run reached the gateway with nothing injected"
  fi
}

echo "--- negative control 3a: a lease the broker says expired (410) injects no usable credential"
lease_control expired-run

echo "--- negative control 3b: a run the broker does not know (404) injects no usable credential"
lease_control absent-run

echo "--- negative control 3c: a lease that names no account is refused, never an empty x-account-id"
before=$(count_for noaccount-run)
code=$(request -H 'X-Run-Id: noaccount-run' -H "$authn" -d '{"model":"spike-model","messages":[]}')
after=$(count_for noaccount-run)
echo "spike: noaccount-run -> HTTP $code, gateway requests for noaccount-run before=$before after=$after"
case "$code" in
  401|403) ;;
  *) fail "expected 401 or 403 for a lease with no account id, got $code" ;;
esac
[ "$after" -eq "$before" ] || fail "a lease with no account id reached the gateway"
echo "PASS: a lease that names no account was refused at the authorizer ($code) and the gateway saw no request"

# The person's balance (ANR-669): zero and an account the ledger does not know are refused with 402 before any model
# call; a balance the ledger could not read is refused too (403 — an outage is not a person's spending, so it is not
# misreported as 402), and the gateway's journal does not move in any of them.
budget_refused() { # budget_refused RUN LABEL EXPECTED_CODE
  run=$1
  label=$2
  want=$3
  before=$(count_for "$run")
  code=$(request -H "X-Run-Id: $run" -H "$authn" -d '{"model":"spike-model","messages":[]}')
  after=$(count_for "$run")
  echo "spike: $label -> HTTP $code, body $(cat "$body"), gateway requests for $run before=$before after=$after"
  [ "$code" = "$want" ] || fail "$label: expected $want, got $code"
  [ "$after" -eq "$before" ] || fail "$label was refused but still reached the gateway"
  echo "PASS: $label was refused with $code and the gateway saw no request"
}

echo "--- the balance: a person at zero is refused with 402, before the model"
budget_refused zero-run "a person whose balance is zero" 402
printf '%s' "$(cat "$body")" | jq -e '.error == "model_locked"' >/dev/null \
  || fail "a zero balance did not answer model_locked"
echo "PASS: the 402 names model_locked, the refusal the run parks on"

echo "--- the balance: an account the ledger has never seen fails closed the same way (it answers 200 with zero)"
budget_refused unknown-run "an account the ledger does not know" 402
printf '%s' "$(cat "$body")" | jq -e '.error == "model_locked"' >/dev/null \
  || fail "an unknown account did not answer the same 402/model_locked as zero"
echo "PASS: unknown and zero are one refusal shape — no credit, never a made-up pass or a 404"

echo "--- the balance: a ledger that cannot answer is a refusal, not a zero (403 budget_unavailable)"
budget_refused unreadable-run "a ledger answering 503 budget_unavailable" 403

echo "--- the balance: a client cannot claim its own, any more than its own Authorization"
before=$(count_for zero-run)
code=$(request -H 'X-Run-Id: zero-run' -H "$authn" -H 'x-budget-remaining: 42' \
  -d '{"model":"spike-model","messages":[]}')
after=$(count_for zero-run)
echo "spike: a zero balance with a client-sent x-budget-remaining: 42 -> HTTP $code, gateway requests before=$before after=$after"
[ "$code" = 402 ] || fail "a client-sent balance survived the injector (got $code)"
[ "$after" -eq "$before" ] || fail "a client-sent balance let a zero-balance run reach the gateway"
echo "PASS: the balance is the ledger's, replaced like the credential and never the client's"

echo "spike: all checks passed"
