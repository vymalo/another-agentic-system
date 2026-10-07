#!/usr/bin/env sh
# System-level test of the browser web's own sign-in (ADR 0054): the web is a public client of the issuer, holds DPoP-bound tokens and
# an offline refresh token, and the orchestrator verifies the proofs itself. The whole chain goes through the REAL edge, oauth2-proxy
# beside it, the REAL orchestrator and the mock issuer, as the web's requests do, with no browser: dev/browser-auth/flow.mjs does what the
# web does with `oauth4webapi` (node's own WebCrypto and fetch, no dependencies).
#
#   dev/browser-auth-e2e.sh
#
# Start the stack WITH the override first (dev/compose.browser-auth.yaml: the orchestrator's `auth.dpop` and `auth.browser`, the edge's
# DPoP routes, the issuer's origins; the orchestrator image must have ADR 0054, which CI builds from the checkout):
#
#   docker compose -f compose.yaml -f dev/compose.browser-auth.yaml --profile app up -d --build --wait
#
# `dev/e2e-all.sh` does not run it: it needs the override (another shape of the stack, like `devcontainer` and `split`), which CI starts
# in .github/workflows/coder-e2e.yml. dev/README.md, "Tokens in the browser", is the guide.
#
# What it asserts (each is an ok or FAIL line of the flow; the flow's header says the rest):
#   * the issuer: `GET /api/public/auth` through the edge, with no sign-in (and with a bogus Authorization and an identity header, which the
#     edge strips), is `{issuer, clientId, scope}` and nothing else; the issuer's discovery names revocation, end_session and DPoP with ES256;
#   * the sign-in: authorize with PKCE as a person named by login_hint (without PKCE it is refused), the code redeemed WITH a DPoP proof
#     (without one, with another htu or a wrong verifier it is refused); the access token is bound to the key (`cnf.jkt`), is the person's
#     and lasts minutes; the ID token has the nonce; the scope has offline_access;
#   * DPoP through the edge: `GET /api/me` with `Authorization: DPoP` and a proof is 200 and the person's, an identity header beside it
#     changes nothing; the same token as Bearer is 401 (with and without its proof beside it); no proof is 401; a replayed proof, a proof for another
#     path, origin or method, an old one, one from the future, another token's `ath`, another key's (a stolen token), a `typ` that is not
#     dpop+jwt and an `alg` that is not asymmetric are all 401, and the refusal names DPoP; a token with no `cnf` sent as DPoP is 401 while the same
#     one as Bearer is the script of before (200);
#   * AG-UI through the edge: a run with DPoP (`POST /agui/agents/mock-coder`) is a 200 event stream, the thread is the person's and ends done,
#     `GET /agui/threads/{id}/connect` with DPoP follows it, and another person's DPoP token gets 404 for both;
#   * the refresh token is used once: a refresh with the key gives new tokens (bound to the same key, good at the API); the OLD token used
#     again is invalid_grant and the NEW one is dead too (the whole session is revoked); a refresh with another key, or with no proof, is
#     invalid_grant and uses nothing up; revocation (RFC 7009) needs a proof of the key, and a revoked token cannot refresh; end_session redirects.
#   * the pages: `GET /` and `/auth/callback` are served with no sign-in (no redirect to oauth2-proxy), while `/oauth2/start` still goes to the
#     issuer and a plain Bearer token (a script of before) still works.
#
# The flow runs inside the compose network (in the `mock-oidc` container: it has node, and `edge` and `mock-oidc` resolve there), because the
# issuer's address is `http://mock-oidc:8080` there (the `iss` of every token and what the orchestrator is configured with: a browser on the host cannot
# use it, dev/README.md says what to do for one). The proofs name PUBLIC_ORIGIN, one of `auth.dpop.publicOrigins`.
#
# It prints one ok or FAIL line per check and exits 1 if any failed.
#
# Environment (defaults match compose.yaml on one machine; run it from anywhere, it changes to the repository's root):
#   BASE_URL       http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge` as the host reaches it (the page checks use it)
#   COMPOSE_CMD    docker compose -f compose.yaml -f dev/compose.browser-auth.yaml --profile app   how to reach the stack's `mock-oidc`
#   EDGE_URL       http://edge:8080   the edge as the flow reaches it, inside the network
#   PUBLIC_ORIGIN  http://edge:8080   the origin the flow's proofs name; one of `auth.dpop.publicOrigins` of dev/orchestrator.browser-auth.yaml
#   USER_EMAIL     dev@example.com (OTHER_EMAIL, someone-else@example.com, is the other person); AGENT_ID mock-coder; TIMEOUT 60
#   OIDC_REACH     (unset) where the flow reaches the issuer, to run it on the host (see the flow's header)
# Needs curl and docker compose (and nothing else: the flow runs in the container).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
cd "$here/.."
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "browser auth e2e passed"; else echo "browser auth e2e FAILED"; exit 1; fi
}

command -v curl >/dev/null 2>&1 || { echo "curl is required and was not found" >&2; exit 2; }

# --- the two configurations agree (no docker needed) ----------------------------------------------------------
if sh "$here/check-browser-auth-config.sh" >/dev/null 2>&1; then
  ok "dev/orchestrator.browser-auth.yaml is dev/orchestrator.yaml plus its marked blocks"
else
  bad "dev/orchestrator.browser-auth.yaml and dev/orchestrator.yaml disagree (dev/check-browser-auth-config.sh says where)"
fi

# --- is the stack there, with the override? --------------------------------------------------------------------
if ! curl -fsS --max-time 10 "$base/readyz" >/dev/null 2>&1; then
  cat >&2 <<EOF
Nothing answers at $base/readyz. Start the stack WITH the override first:

  docker compose -f compose.yaml -f dev/compose.browser-auth.yaml --profile app up -d --build --wait

or point BASE_URL (or EDGE_PORT) at the edge you started. See dev/README.md, "Tokens in the browser".
EOF
  exit 2
fi
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 "$base/api/public/auth" || true)
if [ "$code" != 200 ]; then
  cat >&2 <<EOF
GET $base/api/public/auth answered HTTP ${code:-none}, not 200: this stack is not running the override, or its orchestrator has no
\`auth.browser\` (an image from before ADR 0054 refuses the configuration: look at \`docker compose logs orchestrator\`). Start it with

  docker compose -f compose.yaml -f dev/compose.browser-auth.yaml --profile app up -d --build --wait
EOF
  exit 2
fi

# --- the pages need no sign-in; the cookie path of before is still there ---------------------------------------
status_of() { curl -s -o /dev/null -w '%{http_code}' --max-time 30 "$@" || true; }
for page in / "/auth/callback?code=none&state=none"; do
  got=$(status_of "$base$page")
  case $got in
    302 | 301 | 401 | 403 | 000 | "") bad "GET $page is HTTP ${got:-none}: the web's pages are meant to be served with no sign-in" ;;
    *) ok "GET $page is served with no sign-in (HTTP $got, no redirect to oauth2-proxy)" ;;
  esac
done
location=$(curl -s -o /dev/null -D - --max-time 30 "$base/oauth2/start?rd=/" | sed -n 's/^[Ll]ocation: *//p' | tr -d '\r')
case $location in
  */authorize\?*client_id=dev-chat*) ok "/oauth2/start still sends a browser to the issuer, as the confidential client (the cookie path of before)" ;;
  *) bad "/oauth2/start did not redirect to the issuer's authorize with client_id=dev-chat: '${location:-no location}'" ;;
esac

# --- the flow, inside the compose network -----------------------------------------------------------------------
compose=${COMPOSE_CMD:-docker compose -f compose.yaml -f dev/compose.browser-auth.yaml --profile app}
if ! command -v docker >/dev/null 2>&1; then
  bad "docker is required to run the flow in the mock-oidc container (or run it on the host: see the header of dev/browser-auth/flow.mjs)"
  finish
fi
# shellcheck disable=SC2086 # COMPOSE_CMD is words on purpose
if ! $compose exec -T mock-oidc test -f /browser-auth/flow.mjs; then
  bad "the mock-oidc container has no /browser-auth/flow.mjs: is the stack running with -f dev/compose.browser-auth.yaml (and 'mock-oidc' up)?"
  finish
fi
echo "-- dev/browser-auth/flow.mjs, in the mock-oidc container"
flow_rc=0
# shellcheck disable=SC2086
$compose exec -T \
  -e "EDGE_URL=${EDGE_URL:-http://edge:8080}" \
  -e "PUBLIC_ORIGIN=${PUBLIC_ORIGIN:-http://edge:8080}" \
  -e "USER_EMAIL=${USER_EMAIL:-dev@example.com}" \
  -e "OTHER_EMAIL=${OTHER_EMAIL:-someone-else@example.com}" \
  -e "AGENT_ID=${AGENT_ID:-mock-coder}" \
  -e "TIMEOUT=${TIMEOUT:-60}" \
  ${OIDC_REACH:+-e "OIDC_REACH=$OIDC_REACH"} \
  mock-oidc node /browser-auth/flow.mjs || flow_rc=$?
if [ "$flow_rc" -eq 0 ]; then ok "the flow passed"; else bad "the flow failed (exit $flow_rc): its FAIL lines are above"; fi

finish
