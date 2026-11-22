#!/usr/bin/env sh
# The proof (sys#334 S9): a client sends `X-Run-Id` and its own `Authorization`; the external
# authorizer adds the upstream `Authorization`; the mock model must see the added one. A request
# without `X-Run-Id` is refused and must reach no model.
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

# Wait for the port-forward: any answer but none at all means the tunnel is up.
for _ in $(seq 1 30); do
  code=$(request -H 'X-Run-Id: warmup' -d '{"model":"spike-model","messages":[]}' || true)
  case "$code" in ""|000) sleep 1 ;; *) break ;; esac
done

before=$(count)

code=$(request -H 'X-Run-Id: run-1' -H 'Authorization: Bearer client-sent' \
  -d '{"model":"spike-model","messages":[{"role":"user","content":"hi"}]}')
echo "spike: with X-Run-Id: HTTP $code"
cat "$dir/.body"
echo
[ "$code" = 200 ] || { echo "spike.sh: expected 200" >&2; exit 1; }

content=$(jq -r '.choices[0].message.content' "$dir/.body")
printf '%s\n' "$content" | jq .
got_auth=$(printf '%s\n' "$content" | jq -c '.received.Authorization')
got_run=$(printf '%s\n' "$content" | jq -c '.received["X-Run-Id"]')
got_user=$(printf '%s\n' "$content" | jq -c '.received["X-Auth-User"]')
printf '%s\n' "$got_auth" | grep -q 'injected-by-extauth' \
  || { echo "FAIL: the injected Authorization did not reach the model" >&2; exit 1; }
printf '%s\n' "$got_run" | grep -q 'run-1' \
  || { echo "FAIL: X-Run-Id did not reach the model" >&2; exit 1; }
echo "PASS: the ext-auth Authorization reached the model"
echo "      Authorization: $got_auth"
echo "      X-Run-Id: $got_run"
echo "      X-Auth-User: $got_user"

# The negative: no X-Run-Id is refused, and the refusal must not reach the model.
code=$(request -H 'Authorization: Bearer client-sent' -d '{"model":"spike-model","messages":[]}')
echo "spike: without X-Run-Id: HTTP $code"
case "$code" in
  401|403) ;;
  *) echo "FAIL: expected 401 or 403, got $code" >&2; exit 1 ;;
esac

after=$(count)
echo "spike: model requests before=$before after=$after"
[ "$((after - before))" -eq 1 ] \
  || { echo "FAIL: the refused request reached the model (expected exactly 1 new request)" >&2; exit 1; }
echo "PASS: the refused request reached no model"
