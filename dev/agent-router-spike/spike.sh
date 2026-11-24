#!/usr/bin/env sh
# The proof (sys#334 S9, ANR-662), with the real Authorino over gRPC and WireMock as the model gateway: the agent
# sends a run id and its own `Authorization`; Authorino authenticates it, asks the broker's run-lease route for the
# run's key and account (with the injector's own token), and adds `Authorization` and `x-account-id` upstream. The
# gateway's request journal must see exactly one `Authorization`, the leased one — never the client's — and the
# account id the lease named. Three negative controls make the check able to fail: without the SecurityPolicy the
# client's header arrives, without a run id nothing reaches the gateway, and a refused lease injects nothing (or
# stops the request) rather than a usable credential.
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

echo "--- the agent's request: its own bearer is not the model key, and the lease names the account"
before=$(count_for run-1)
code=$(request -H 'X-Run-Id: run-1' -H 'Authorization: Bearer client-sent-DO-NOT-PASS' \
  -d '{"model":"spike-model","messages":[{"role":"user","content":"hi"}]}')
after=$(count_for run-1)
echo "spike: HTTP $code, gateway requests for run-1 before=$before after=$after"
[ "$code" = 200 ] || { cat "$body"; echo; fail "expected 200"; }
[ "$after" -eq $((before + 1)) ] || fail "expected exactly one new request for run-1 at the gateway"

n=$(hcount run-1 authorization)
v=$(hval run-1 authorization)
a=$(hval run-1 x-account-id)
echo "gateway saw: Authorization occurrences=$n value=[$v]"
echo "gateway saw: x-account-id=[$a]"
[ "$n" -eq 1 ] || fail "expected exactly one Authorization at the gateway"
[ "$v" = "Bearer $marker" ] || fail "the leased Authorization did not reach the gateway (got [$v])"
[ "$a" = "$account" ] || fail "the leased account id did not reach the gateway (got [$a])"
case "$v" in *client-sent-DO-NOT-PASS*) fail "the client's own Authorization survived" ;; esac
echo "PASS: the lease's key and account id reached the gateway; the agent's own Authorization did not"

echo "--- the broker saw the lookup, with the injector's token, keyed by the run, and minted that account"
last=$(kc exec deploy/spike-broker -- python -c \
  'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:8080/__last").read().decode())')
echo "broker: $last"
printf '%s' "$last" | jq -e '.run == "run-1" and .injectorToken == true and .accountId == "'"$account"'"' >/dev/null \
  || fail "the broker was not asked for run-1 with the injector's token"
echo "PASS: Authorino asked the broker for the run's lease with its own token, and the lease named $account"

echo "--- negative control 1: with the SecurityPolicy removed, the client's own header arrives"
kc delete securitypolicy spike-extauth --ignore-not-found >/dev/null
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

echo "--- restoring the policy"
kc apply -f "$dir/manifests/31-route-policy.yaml" >/dev/null
for _ in $(seq 1 30); do
  code=$(request -H 'X-Run-Id: run-1' -H 'Authorization: Bearer client-sent-DO-NOT-PASS' \
    -d '{"model":"spike-model","messages":[]}')
  if [ "$code" = 200 ] && [ "$(hval run-1 authorization)" = "Bearer $marker" ]; then break; fi
  sleep 2
done
[ "$(hval run-1 authorization)" = "Bearer $marker" ] || fail "the policy did not come back"
echo "PASS: the policy is back and the leased headers replace the client's again"

echo "--- negative control 2: without a run id, nothing reaches the gateway"
before=$(count_without_run)
code=$(request -H 'Authorization: Bearer client-sent-DO-NOT-PASS' -d '{"model":"spike-model","messages":[]}')
after=$(count_without_run)
echo "spike: without X-Run-Id: HTTP $code, gateway requests without a run before=$before after=$after"
case "$code" in
  401|403) ;;
  *) fail "expected 401 or 403, got $code" ;;
esac
[ "$after" -eq "$before" ] || fail "the refused request reached the gateway"
echo "PASS: the refused request reached no gateway"

# A lease the broker refuses (410 lease_expired, 404 run_not_bound): either the authorizer stops the request, or it
# reaches the gateway with nothing injected. Say which happened, with the observable.
lease_control() { # lease_control RUN
  run=$1
  before=$(count_for "$run")
  code=$(request -H "X-Run-Id: $run" -d '{"model":"spike-model","messages":[]}')
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

echo "spike: all checks passed"
