#!/usr/bin/env sh
# Prints the header line a script sends, to be EMAIL, to the API and the AG-UI routes of the local stack. The scripts of this
# directory take it once, near the top, and send it with `-H "$auth"`:
#
#   auth=$(sh "$(dirname "$0")/auth-header.sh" "$email")
#
# The stack puts oauth2-proxy in front of the API (ADR 0033), and the orchestrator is an OAuth2 resource server (`auth.mode: jwt`):
# a request is somebody only with `Authorization: Bearer <JWT>`. This script gets one from the mock issuer (`mock-oidc`) with a
# client_credentials request that names the user, an extension of that mock for scripts, and prints `Authorization: Bearer <token>`.
# oauth2-proxy skips a bearer token it can verify (`--skip-jwt-bearer-tokens`, the mock's issuer is one of
# `--extra-jwt-issuers`), and the orchestrator validates the same token again. A token lasts an hour (`TOKEN_TTL_SECS` of the mock).
#
# Environment (all optional; the defaults match compose.yaml on one machine):
#   AUTH_EMAIL          the user, when no argument names one (dev@example.com). The users are dev/mock-oidc/users.json
#   OIDC_URL            http://127.0.0.1:${MOCK_OIDC_PORT:-8099}   where the mock issuer is reached from the host
#   OIDC_CLIENT_ID      dev-chat                                  the client of the mock (`CLIENT_ID` of the service)
#   OIDC_CLIENT_SECRET  dev-client-secret                         its secret (`CLIENT_SECRET`); a dummy
#   OIDC_AUDIENCE       (the client id)  the `aud` of the token; another one is refused by oauth2-proxy and by the orchestrator
#   AUTH_BEARER         a token you already have: printed as it is, nothing is asked of the issuer (a real issuer's, say)
#   AUTH_MODE           bearer (the default) or proxy-header: `X-Auth-Request-Email: <email>`, for an orchestrator started
#                       by hand with `auth.mode: proxy_header` (dev/README.md, "The orchestrator on the host")
#
# Exit status: 0 and one line on stdout, or 1 with the reason on stderr (nothing on stdout). Needs curl and jq.
set -eu

email=${1:-${AUTH_EMAIL:-dev@example.com}}

if [ "${AUTH_MODE:-bearer}" = proxy-header ]; then
  echo "X-Auth-Request-Email: $email"
  exit 0
fi
if [ -n "${AUTH_BEARER:-}" ]; then
  echo "Authorization: Bearer $AUTH_BEARER"
  exit 0
fi

oidc=${OIDC_URL:-http://127.0.0.1:${MOCK_OIDC_PORT:-8099}}
oidc=${oidc%/}
set -- --data-urlencode grant_type=client_credentials --data-urlencode "user=$email"
[ -z "${OIDC_AUDIENCE:-}" ] || set -- "$@" --data-urlencode "audience=$OIDC_AUDIENCE"

# The secret goes by `-u`, which a process listing shows: a dummy of a mock, and nothing here is a real credential.
if ! answer=$(curl -fsS --max-time 20 -u "${OIDC_CLIENT_ID:-dev-chat}:${OIDC_CLIENT_SECRET:-dev-client-secret}" "$@" "$oidc/token" 2>&1); then
  echo "auth-header.sh: no token for $email from $oidc/token: $answer (is the stack up: docker compose --profile app up -d --wait mock-oidc?)" >&2
  exit 1
fi
token=$(printf '%s' "$answer" | jq -r '.access_token // empty' 2>/dev/null || true)
if [ -z "$token" ]; then
  echo "auth-header.sh: $oidc/token answered no access_token for $email: $(printf '%s' "$answer" | head -c 200)" >&2
  exit 1
fi
echo "Authorization: Bearer $token"
