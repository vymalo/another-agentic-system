#!/usr/bin/env sh
# The proof (sys#334 S9), with the real Authorino over gRPC: the agent sends a run id and its own
# `Authorization`; Authorino authenticates it, asks the broker's run-lease route for the run's key
# (with the injector's own token), and adds that key upstream. The mock model must see exactly one
# `Authorization`, the leased one — never the client's. Two negative controls make the check able to
# fail: without the SecurityPolicy the client's header arrives, and without a run id nothing reaches
# the model.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
ctx=kind-agent-router-spike
kc() { kubectl --context "$ctx" "$@"; }

svc=$(kc -n envoy-gateway-system get svc -l gateway.envoyproxy.io/owning-gateway-name=spike \
  -o jsonpath='{.items[0].metadata.name}')
[ -n "$svc" ] || { echo "spike.sh: no gateway service; run up.sh first" >&2; exit 1; }

kc -n envoy-gateway-system port-forward "svc/$svc" 18080:80 >/dev/null 2>&1 &
pf=$!
trap 'kill $pf 2>/dev/null || true' EXIT

request() { # request ARGS...
  curl -sS -o "$dir/.body" -w '%{http_code}' -X POST http://127.0.0.1:18080/v1/chat/completions \
    -H 'content-type: application/json' -H 'x-ai-eg-model: spike-model' "$@"
}

count() {
  kc exec deploy/spike-mock -- python -c \
    'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:8080/__count").read().decode())' \
    | jq .count
}

content() { jq -r '.choices[0].message.content' "$dir/.body"; }
auth_values() { content | jq -r '.received.Authorization // [] | join(" | ")'; }
auth_count() { content | jq -r '.received.Authorization // [] | length'; }

# Wait for the port-forward: any answer but none at all means the tunnel is up.
for _ in $(seq 1 30); do
  code=$(request -H 'X-Run-Id: warmup' -d '{"model":"spike-model","messages":[]}' || true)
  case "$code" in ""|000) sleep 1 ;; *) break ;; esac
done

marker=anr250-lease-marker-7f3c-run-1

echo "--- the agent's request: its own bearer is not the model key"
before=$(count)
code=$(request -H 'X-Run-Id: run-1' -H 'Authorization: Bearer client-sent-DO-NOT-PASS' \
  -d '{"model":"spike-model","messages":[{"role":"user","content":"hi"}]}')
echo "spike: HTTP $code"
[ "$code" = 200 ] || { cat "$dir/.body"; echo; echo "spike.sh: expected 200" >&2; exit 1; }

n=$(auth_count)
v=$(auth_values)
echo "Authorization occurrences: $n -> [$v]"
[ "$n" -eq 1 ] || { echo "FAIL: expected exactly one Authorization at the model" >&2; exit 1; }
printf '%s' "$v" | grep -q "$marker" \
  || { echo "FAIL: the leased Authorization did not reach the model" >&2; exit 1; }
printf '%s' "$v" | grep -q 'client-sent-DO-NOT-PASS' \
  && { echo "FAIL: the client's own Authorization survived" >&2; exit 1; }
echo "PASS: the leased Authorization reached the model; the agent's own did not"

echo "--- the broker saw the lookup, with the injector's token, keyed by the run"
last=$(kc exec deploy/spike-broker -- python -c \
  'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:8080/__last").read().decode())')
echo "broker: $last"
printf '%s' "$last" | jq -e '.run == "run-1" and .injectorToken == true' >/dev/null \
  || { echo "FAIL: the broker was not asked for run-1 with the injector's token" >&2; exit 1; }
echo "PASS: Authorino asked the broker for the run's lease with its own token"

echo "--- negative control 1: with the SecurityPolicy removed, the client's own header arrives"
kc delete securitypolicy spike-extauth --ignore-not-found >/dev/null
# The policy is gone once a request that carries the client's marker reaches the model with it.
for _ in $(seq 1 15); do
  code=$(request -H 'X-Run-Id: run-1' -H 'Authorization: Bearer client-sent-NO-EXTAUTH' \
    -d '{"model":"spike-model","messages":[]}')
  if auth_values | grep -q 'client-sent-NO-EXTAUTH'; then break; fi
  sleep 2
done
v=$(auth_values)
echo "Authorization at the model without the policy: [$v]"
printf '%s' "$v" | grep -q 'client-sent-NO-EXTAUTH' \
  || { echo "FAIL: the negative control did not reach the model (the check cannot fail)" >&2; exit 1; }
echo "PASS: the check can fail — the client's header is what arrives without the injector"

echo "--- restoring the policy"
kc apply -f "$dir/manifests/31-route-policy.yaml" >/dev/null
for _ in $(seq 1 30); do
  code=$(request -H 'X-Run-Id: run-1' -H 'Authorization: Bearer client-sent-DO-NOT-PASS' \
    -d '{"model":"spike-model","messages":[]}')
  if [ "$code" = 200 ] && auth_values | grep -q "$marker"; then break; fi
  sleep 2
done
auth_values | grep -q "$marker" \
  || { echo "FAIL: the policy did not come back" >&2; exit 1; }
echo "PASS: the policy is back and the leased header replaces the client's again"

echo "--- negative control 2: without a run id, nothing reaches the model"
before=$(count)
code=$(request -H 'Authorization: Bearer client-sent-DO-NOT-PASS' -d '{"model":"spike-model","messages":[]}')
echo "spike: without X-Run-Id: HTTP $code"
case "$code" in
  401|403) ;;
  *) echo "FAIL: expected 401 or 403, got $code" >&2; exit 1 ;;
esac
after=$(count)
echo "spike: model requests before=$before after=$after"
[ "$((after - before))" -eq 0 ] \
  || { echo "FAIL: the refused request reached the model" >&2; exit 1; }
echo "PASS: the refused request reached no model"

echo "spike: all checks passed"
