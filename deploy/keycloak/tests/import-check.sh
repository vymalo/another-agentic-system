#!/usr/bin/env sh
# Imports every export of deploy/keycloak into a real Keycloak, the way the owner does in the admin console (Clients → Import
# client, then Realm settings → Partial import), into a realm `vymalo` made for the check, and reads back what matters of the
# web's client (ADR 0054). A client whose description is longer than Keycloak's column (255) is refused with a 500 there, which
# is how the first version of these files would have failed for the owner (found 2026-10-09).
#
#   docker run -d --name kc -p 127.0.0.1:8180:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
#     quay.io/keycloak/keycloak:26.6.1 start-dev
#   sh deploy/keycloak/tests/import-check.sh
#
# Environment: KC_URL (http://127.0.0.1:8180), KC_ADMIN and KC_PASSWORD (admin, admin). Needs curl and jq. The realm `vymalo` of
# that Keycloak is deleted and made again: never point it at a Keycloak that matters.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
dir="$here/.."
kc=${KC_URL:-http://127.0.0.1:8180}
fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

i=0
until curl -fsS -o /dev/null "$kc/realms/master"; do
  i=$((i + 1))
  [ "$i" -lt 90 ] || { echo "Keycloak at $kc did not answer"; exit 1; }
  sleep 2
done

token=$(curl -fsS "$kc/realms/master/protocol/openid-connect/token" \
  -d grant_type=password -d client_id=admin-cli \
  --data-urlencode "username=${KC_ADMIN:-admin}" --data-urlencode "password=${KC_PASSWORD:-admin}" | jq -r .access_token)
api() { curl -sS -o /tmp/import-check.out -w '%{http_code}' -H "Authorization: Bearer $token" -H 'Content-Type: application/json' "$@"; }

api -X DELETE "$kc/admin/realms/vymalo" > /dev/null
status=$(api -X POST "$kc/admin/realms" -d '{"realm":"vymalo","enabled":true}')
[ "$status" = 201 ] || { echo "could not make the realm: $status $(cat /tmp/import-check.out)"; exit 1; }

for f in client-another-agentic.json client-another-agentic-cli.json client-another-agentic-web.json; do
  status=$(api -X POST "$kc/admin/realms/vymalo/clients" --data-binary "@$dir/$f")
  if [ "$status" = 201 ]; then ok "Import client: $f"; else bad "Import client: $f answered $status $(cat /tmp/import-check.out)"; fi
done
status=$(api -X POST "$kc/admin/realms/vymalo/partialImport" --data-binary "@$dir/roles-and-groups.json")
if [ "$status" = 200 ] && [ "$(jq -r .skipped /tmp/import-check.out)" = 0 ]; then
  ok "Partial import: roles-and-groups.json ($(jq -r .added /tmp/import-check.out) added)"
else
  bad "Partial import: roles-and-groups.json answered $status $(cat /tmp/import-check.out)"
fi

api "$kc/admin/realms/vymalo/clients?clientId=another-agentic-web" > /dev/null
web=$(cat /tmp/import-check.out)
check() { if printf '%s' "$web" | jq -e ".[0] | $2" > /dev/null 2>&1; then ok "another-agentic-web: $1"; else bad "another-agentic-web: $1"; fi; }
check "public, standard flow only" '.publicClient and .standardFlowEnabled and (.directAccessGrantsEnabled | not) and (.implicitFlowEnabled | not)'
check "PKCE S256 and DPoP-bound tokens" '.attributes["pkce.code.challenge.method"] == "S256" and .attributes["dpop.bound.access.tokens"] == "true"'
check "a 5-minute access token" '.attributes["access.token.lifespan"] == "300"'
check "offline_access is an optional scope" '.optionalClientScopes | index("offline_access")'
check "the audience another-agentic is in the access token and NOT in the ID token (an ID token carries no key binding)" \
  '.protocolMappers[] | select(.protocolMapper == "oidc-audience-mapper") | .config["access.token.claim"] == "true" and .config["id.token.claim"] == "false"'
check "the roles claim agentic_roles from another-agentic's client roles" \
  '.protocolMappers[] | select(.config["claim.name"] == "agentic_roles") | .config["usermodel.clientRoleMapping.clientId"] == "another-agentic"'

api -X DELETE "$kc/admin/realms/vymalo" > /dev/null
rm -f /tmp/import-check.out
if [ "$fail" != 0 ]; then
  echo "an export does not import as written"
  exit 1
fi
echo "the Keycloak exports import"
