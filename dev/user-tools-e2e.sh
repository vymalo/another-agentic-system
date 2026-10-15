#!/usr/bin/env sh
# System-level test of the MCP servers people add themselves (ADR 0046, slice 4b): the routes `POST`, `GET`, `PATCH` and `DELETE` of
# /api/user-tool-servers and the listing GET /api/tool-servers, through the real edge, with the sealed store and the credential broker of the
# orchestrator over the stack's Postgres. Nothing connects to the person's server in this slice (attaching one to a thread, and the relay's
# lease, are slice 5; an OAuth server's authorization is slice 4c), so the server is the stack's `mock-mcp-search` by its name on the
# compose network, which `userToolServers.egress` of dev/orchestrator.yaml names as the one host of this stack.
#
#   dev/user-tools-e2e.sh
#
# Start the `app` profile first (docker compose --profile app up -d --build --wait). It needs no agent: it speaks only to the API. The users are
# those of dev/mock-oidc/users.json: `tool-maker@example.com` and `tool-maker-2@example.com` hold `tools.user.add` (the role `tool-maker` of
# dev/orchestrator.yaml, the only role that does), `dev@example.com` is a user without it, `someone-else@example.com` another user.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the one public path (ADR 0046, slice 4c; the edge lets exactly `GET /api/user-tool-servers/oauth/callback` through with no sign-in):
#     with NO credential, that path with a made-up `state` answers the orchestrator's fixed 400 page ("This link is not valid") with its
#     `Content-Security-Policy` and the other headers of the page, while `GET /api/user-tool-servers`, `HEAD` of the callback and `POST` of the
#     callback are 401 (the edge's `forward_auth`), and so is every near miss of the path that Caddy's matcher reads as the same path
#     (another case, a doubled slash, an encoded `%2F`, a `..` segment, a trailing slash): the matcher is wider than the router, which matches
#     exactly, so the near miss reaches the orchestrator and is answered by its identity layer, never by the callback;
#   * GET /api/me lists `tools.user.add` for a tool-maker and not for dev or admin (no role holds it by default, `admin` does not imply it);
#   * dev, without the permission: POST is 403 `forbidden`, the listing is 200 and empty with `limit` 8 and the four kinds;
#   * a tool-maker adds a server that wants nothing (`none`: `connected`, `ready`), one with a bearer token and one with an API key in the header
#     `X-Api-Key` (each: 201, `connected`, `ready`, `hasCredential` true, `own` true, and the secret nowhere in the answer);
#   * GET /api/user-tool-servers lists the three by name with their URL, kind, header name, state and revision; GET /api/tool-servers lists the
#     deployment's `websearch` first, as it always did (no `own`, no `status`), then the three of the person, each with `own`, `status: ready`
#     and no URL, kind or revision;
#   * PATCH: a new name with the listed `revision` is 200 and a new revision, the same old revision again is 409 `stale_revision`, a new token
#     re-keys, a new URL revokes the credential and leaves the server `added` (`needs_auth`, `not_connected`, `hasCredential` false), a new key
#     with it makes it `connected` again; a body with nothing to change, an unknown member and a secret for a server that wants none are 400;
#   * the guard: a host the allow list does not name, a URL with user information and one with a query are 400 `url_refused` with the guard's
#     words, never the precise rule;
#   * another person with the permission (`tool-maker-2`) lists none of these, and PATCH and DELETE of an id of the first are 404, the answer for an
#     id that never existed; a user without it (`someone-else`) is refused PATCH 403 and DELETE 404; dev reads its own (none) and deletes nothing of
#     another's;
#   * a person adds eight and the ninth is 409 `too_many_servers`;
#   * AT REST: the Postgres table `user_secrets` holds a row for each credential (sealed) and neither it nor `user_tool_servers` contains a
#     secret; no response of this script contained one; and `docker compose logs orchestrator` contains none and no URL path (the last two need
#     docker compose, else they are reported as skipped). The log check reads what the stack logs, which is the default `info,rmcp=warn`
#     (compose sets no RUST_LOG): it CANNOT SEE a leak at `debug` or `trace`, and says nothing about the request spans. That is covered by the
#     Rust test `a_secret_typed_for_a_server_is_in_no_response_no_log_no_event_and_no_definition` in
#     orchestrator/crates/api/tests/user_tools_secret.rs, which captures every level up to `trace` with the span events and finds a canary;
#   * DELETE: 204 for each, 404 the second time, the list empty, and the sealed rows gone with their servers.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL     http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   OIDC_URL     http://127.0.0.1:${MOCK_OIDC_PORT:-8099}, the mock issuer (dev/auth-header.sh reads it, and OIDC_CLIENT_ID and _SECRET)
#   MOCK_HOST    mock-mcp-search, the one host `userToolServers.egress.allow` of dev/orchestrator.yaml names
#
# It deletes the servers of the two tool-makers first (an earlier run that was killed), and again at the end. Needs curl and jq; docker compose
# for the checks of the database and the logs. Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
host=${MOCK_HOST:-mock-mcp-search}
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
api=/api/user-tool-servers

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
note() { echo "skip $1"; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "user tools e2e passed"; else echo "user tools e2e FAILED"; exit 1; fi
}
# expect WHAT GOT WANT: one line, ok when they are equal.
expect() {
  if [ "$2" = "$3" ]; then ok "$1: $3"; else bad "$1: got '$2', want '$3'"; fi
}

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "this script needs $tool" >&2; exit 2; }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
: >"$tmp/seen"

maker="tool-maker@example.com"
maker2="tool-maker-2@example.com"
dev=dev@example.com
admin=admin@example.com
else_=someone-else@example.com

header_of() { sh "$here/auth-header.sh" "$1"; }
no_token() { bad "a token for $1 from the mock issuer (is mock-oidc up? OIDC_URL=${OIDC_URL:-http://127.0.0.1:${MOCK_OIDC_PORT:-8099}})"; finish; }
h_maker=$(header_of "$maker") || no_token "$maker"
h_maker2=$(header_of "$maker2") || no_token "$maker2"
h_dev=$(header_of "$dev") || no_token "$dev"
h_admin=$(header_of "$admin") || no_token "$admin"
h_else=$(header_of "$else_") || no_token "$else_"
ok "the mock issuer gave a token to each of the five users"

# The secrets this run plants, and a path that is only in one URL: none of them may be found where it must not be.
stamp=$(date +%s)-$$
bearer1="SENTINEL-bearer-$stamp-aaaa"
bearer2="SENTINEL-bearer-$stamp-bbbb"
key1="SENTINEL-apikey-$stamp-cccc"
key2="SENTINEL-apikey-$stamp-dddd"
in_path="SENTINEL-path-$stamp"
url="http://$host:8080/mcp"

# call HEADER METHOD PATH [JSON]: the status on stdout, the body in $tmp/body (and added to $tmp/seen, everything the API said).
call() {
  if [ -n "$1" ]; then _auth=$1; else _auth='X-No-Credential: none'; fi
  if [ $# -ge 4 ]; then
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$2" "$base$3" -H "$_auth" -H 'content-type: application/json' -d "$4" 2>"$tmp/err" || true
  else
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$2" "$base$3" -H "$_auth" 2>"$tmp/err" || true
  fi
  cat "$tmp/body" >>"$tmp/seen" 2>/dev/null || true
  echo >>"$tmp/seen"
}
field() { jq -r "$1" "$tmp/body" 2>/dev/null || echo unreadable; }
# anon METHOD PATH [CURL ARGS]: a request that carries no credential of any kind. The status on stdout, the headers in $tmp/headers and
# the body in $tmp/body. `--path-as-is` (a curl argument) sends the path as written, dot segments and all.
anon() {
  _method=$1
  _path=$2
  shift 2
  if [ "$_method" = HEAD ]; then
    curl -sS --max-time 30 -I -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' "$@" "$base$_path" 2>"$tmp/err" || true
  else
    curl -sS --max-time 30 -o "$tmp/body" -D "$tmp/headers" -w '%{http_code}' -X "$_method" "$@" "$base$_path" 2>"$tmp/err" || true
  fi
}
# rheader NAME: the value of a header of the last `anon` answer (the last one when it is repeated), without the line end.
rheader() {
  awk -v want="$1" '
    { sub(/\r$/, "") }
    index($0, ":") > 0 { name = substr($0, 1, index($0, ":") - 1); if (tolower(name) == tolower(want)) { value = substr($0, index($0, ":") + 1); sub(/^ +/, "", value); found = value } }
    END { print found }' "$tmp/headers"
}
add_body() { # add_body NAME URL AUTH_JSON [SECRET]
  if [ $# -ge 4 ]; then
    jq -n --arg name "$1" --arg url "$2" --argjson auth "$3" --arg secret "$4" '{name: $name, url: $url, auth: $auth, secret: $secret}'
  else
    jq -n --arg name "$1" --arg url "$2" --argjson auth "$3" '{name: $name, url: $url, auth: $auth}'
  fi
}

# Whatever an earlier run left of these two people.
clean() {
  for _h in "$h_maker" "$h_maker2"; do
    _status=$(call "$_h" GET "$api")
    [ "$_status" = 200 ] || continue
    for _id in $(jq -r '.servers[].id' "$tmp/body" 2>/dev/null); do
      call "$_h" DELETE "$api/$_id" >/dev/null
    done
  done
}
clean

# --- who holds the permission ----------------------------------------------------------------------------------
perms_of() { jq -r '[.permissions[].permission] | join(",")' "$tmp/body" 2>/dev/null || echo unreadable; }
expect "GET /api/me as a tool-maker" "$(call "$h_maker" GET /api/me)" 200
case ",$(perms_of)," in *,tools.user.add,*) ok "a tool-maker holds tools.user.add" ;; *) bad "a tool-maker lacks tools.user.add: $(perms_of)" ;; esac
expect "GET /api/me as dev" "$(call "$h_dev" GET /api/me)" 200
case ",$(perms_of)," in *,tools.user.add,*) bad "dev holds tools.user.add: $(perms_of)" ;; *) ok "dev does not hold tools.user.add" ;; esac
expect "GET /api/me as admin" "$(call "$h_admin" GET /api/me)" 200
case ",$(perms_of)," in *,tools.user.add,*) bad "admin holds tools.user.add (it does not imply it)" ;; *) ok "admin does not hold tools.user.add (it implies nothing)" ;; esac

# --- the one public path ---------------------------------------------------------------------------------------------
# The browser that comes back from an authorization server has no session, so the edge lets this path through with no sign-in
# (`@oauthCallback` of dev/Caddyfile, GET only). Nothing else under /api/user-tool-servers is public, and the edge's matcher is wider than
# the orchestrator's router: Caddy's `path` matches without regard to case, merges `//`, decodes `%2F` and resolves `..`, and sends the request as
# it came, so a near miss reaches an orchestrator whose router matches exactly. It is fail closed: the near miss is no route there, and the
# fallback of the router is behind the identity layer, so it is a 401, never the callback.
callback=$api/oauth/callback
made_up_state=$(printf '%032d' "$$")
expect "GET the OAuth callback with no credential, a state nobody began" "$(anon GET "$callback?state=$made_up_state")" 400
expect "  Content-Security-Policy" "$(rheader content-security-policy)" "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
expect "  Referrer-Policy" "$(rheader referrer-policy)" no-referrer
expect "  Cache-Control" "$(rheader cache-control)" no-store
expect "  X-Frame-Options" "$(rheader x-frame-options)" DENY
expect "  X-Content-Type-Options" "$(rheader x-content-type-options)" nosniff
case $(rheader content-type) in "text/html"*) ok "  it is a page" ;; *) bad "  the content type is $(rheader content-type)" ;; esac
if grep -q 'This link is not valid' "$tmp/body"; then ok "  the page says the link is not valid"; else bad "  the page is not the fixed one: $(head -c 200 "$tmp/body")"; fi
if grep -q "$made_up_state" "$tmp/body"; then bad "  the page repeats the state"; else ok "  the page repeats nothing of the request"; fi
expect "GET $api with no credential" "$(anon GET "$api")" 401
expect "GET $api/oauth/callbackx with no credential" "$(anon GET "$callback"x)" 401
expect "HEAD of the OAuth callback with no credential" "$(anon HEAD "$callback?state=$made_up_state")" 401
expect "POST of the OAuth callback with no credential" "$(anon POST "$callback?state=$made_up_state")" 401
expect "PUT of the OAuth callback with no credential" "$(anon PUT "$callback?state=$made_up_state")" 401
expect "DELETE of the OAuth callback with no credential" "$(anon DELETE "$callback?state=$made_up_state")" 401
# What Caddy's matcher reads as the callback and the router does not (a trailing slash is not even the matcher's): the identity layer's 401.
for near in \
  "/API/user-tool-servers/oauth/callback" \
  "/api/USER-tool-servers/oauth/Callback" \
  "//api/user-tool-servers/oauth/callback" \
  "/api//user-tool-servers/oauth/callback" \
  "/api/user-tool-servers/oauth%2Fcallback" \
  "/api/user-tool-servers/oauth%2fcallback" \
  "/api/user-tool-servers/oauth/../oauth/callback" \
  "/api/./user-tool-servers/oauth/callback" \
  "/api/user-tool-servers/oauth/callback/"; do
  expect "GET $near with no credential, a state nobody began" "$(anon GET "$near?state=$made_up_state" --path-as-is)" 401
  if grep -q 'This link is not valid' "$tmp/body"; then bad "  it reached the callback"; fi
done

# --- without the permission ---------------------------------------------------------------------------------------
expect "dev: POST $api" "$(call "$h_dev" POST "$api" "$(add_body nope "$url" '{"kind":"none"}')")" 403
expect "dev: the refusal is forbidden" "$(field .code)" forbidden
expect "dev: GET $api" "$(call "$h_dev" GET "$api")" 200
expect "dev: the list is empty, the limit is 8" "$(field '[(.servers | length), .limit] | join(",")')" "0,8"
expect "dev: the kinds the deployment takes" "$(field '.kinds | join(",")')" "none,bearer,api_key,oauth2"

# --- a tool-maker adds three servers ----------------------------------------------------------------------------
expect "POST a server that wants nothing" "$(call "$h_maker" POST "$api" "$(add_body "Plain docs" "$url" '{"kind":"none"}')")" 201
expect "  it is connected and ready, and it is theirs" \
  "$(field '[.state, .status, .own, .hasCredential, .auth.kind] | map(tostring) | join(",")')" "connected,ready,true,false,none"
none_id=$(field .id)
case $none_id in u-??????????) ok "  its id is u- and ten characters: $none_id" ;; *) bad "  its id is $none_id" ;; esac

expect "POST a server with a bearer token" "$(call "$h_maker" POST "$api" "$(add_body "GitHub" "http://$host:8080/$in_path/mcp" '{"kind":"bearer"}' "$bearer1")")" 201
expect "  it is connected, ready and holds a credential" \
  "$(field '[.state, .status, .hasCredential, .auth.kind] | map(tostring) | join(",")')" "connected,ready,true,bearer"
bearer_id=$(field .id)
expect "  the answer does not carry the token" "$(grep -c "$bearer1" "$tmp/body" || true)" 0

expect "POST a server with an API key in a header" \
  "$(call "$h_maker" POST "$api" "$(add_body "Search" "$url" '{"kind":"api_key","header":"X-Api-Key"}' "$key1")")" 201
expect "  it names its header and holds a credential" \
  "$(field '[.state, .hasCredential, .auth.kind, .auth.header] | map(tostring) | join(",")')" "connected,true,api_key,X-Api-Key"
key_id=$(field .id)

# --- reading them --------------------------------------------------------------------------------------------------
expect "GET $api" "$(call "$h_maker" GET "$api")" 200
expect "  the three, by name" "$(field '[.servers[].name] | join(",")')" "GitHub,Plain docs,Search"
expect "  each with its kind, state and a revision" \
  "$(field '[.servers[] | [.auth.kind, .state, (.revision | test("^[0-9]+$") | tostring)] | join(":")] | join(",")')" \
  "bearer:connected:true,none:connected:true,api_key:connected:true"
expect "  a header name is shown, a secret is not" "$(field '[.servers[] | has("secret")] | any | tostring')" false
expect "GET /api/tool-servers" "$(call "$h_maker" GET /api/tool-servers)" 200
expect "  the deployment's server comes first, as it always did" "$(field '.[0] | [.id, has("own"), has("status")] | map(tostring) | join(",")')" "websearch,false,false"
expect "  then the three of the person, each ready and theirs" \
  "$(field '[.[1:][] | [.name, (.own | tostring), .status] | join(":")] | join(",")')" "GitHub:true:ready,Plain docs:true:ready,Search:true:ready"
expect "  the picker shows no URL, kind, header, state or revision of them" \
  "$(field '[.[1:][] | has("url") or has("auth") or has("state") or has("revision")] | any | tostring')" false
expect "another user's picker has only the deployment's" "$(call "$h_dev" GET /api/tool-servers)" 200
expect "  one server" "$(field 'length')" 1

# --- changing them ----------------------------------------------------------------------------------------------
call "$h_maker" GET "$api" >/dev/null
rev1=$(jq -r --arg id "$none_id" '.servers[] | select(.id == $id) | .revision' "$tmp/body")
expect "PATCH a new name, on the revision the listing gave" \
  "$(call "$h_maker" PATCH "$api/$none_id" "$(jq -n --arg rev "$rev1" '{name: "Docs", revision: $rev}')")" 200
expect "  the name changed, the revision too" "$(field '[.name, (.revision != "'"$rev1"'" | tostring)] | join(",")')" "Docs,true"
expect "PATCH on the old revision again" "$(call "$h_maker" PATCH "$api/$none_id" "$(jq -n --arg rev "$rev1" '{name: "Again", revision: $rev}')")" 409
expect "  is stale_revision" "$(field .code)" stale_revision
expect "PATCH a new token (re-keying) of the bearer server" "$(call "$h_maker" PATCH "$api/$bearer_id" "$(jq -n --arg s "$bearer2" '{secret: $s}')")" 200
expect "  it is connected still" "$(field '[.state, .status, .hasCredential] | map(tostring) | join(",")')" "connected,ready,true"
expect "PATCH a new URL of the API key server: the key was for the old one" \
  "$(call "$h_maker" PATCH "$api/$key_id" '{"url":"http://'"$host"':8080/other"}')" 200
expect "  it is added again, wants a key, holds none" \
  "$(field '[.state, .status, .reason, .hasCredential] | map(tostring) | join(",")')" "added,needs_auth,not_connected,false"
expect "  the picker says so" "$(call "$h_maker" GET /api/tool-servers)" 200
expect "  with the reason" "$(field '[.[] | select(.name == "Search") | .status, .reason] | join(",")')" "needs_auth,not_connected"
expect "PATCH a new key with it" "$(call "$h_maker" PATCH "$api/$key_id" "$(jq -n --arg s "$key2" '{secret: $s}')")" 200
expect "  it is connected again" "$(field '[.state, .status, .hasCredential] | map(tostring) | join(",")')" "connected,ready,true"
expect "PATCH with nothing to change" "$(call "$h_maker" PATCH "$api/$none_id" '{}')" 400
expect "PATCH with an unknown member" "$(call "$h_maker" PATCH "$api/$none_id" '{"nam":"x"}')" 400
expect "PATCH a secret for a server that wants none" "$(call "$h_maker" PATCH "$api/$none_id" "$(jq -n --arg s "$bearer1" '{secret: $s}')")" 400
expect "  the 400 does not quote it" "$(grep -c "$bearer1" "$tmp/body" || true)" 0

# --- the guard --------------------------------------------------------------------------------------------------
refused() { # refused WHAT URL: a 400 url_refused whose words are the guard's public reason
  expect "POST $1" "$(call "$h_maker" POST "$api" "$(add_body x "$2" '{"kind":"none"}')")" 400
  expect "  is url_refused" "$(field .code)" url_refused
  case $(field .detail) in
    *private-address* | *no-address* | *deny-list* | *allow-list*) bad "  the answer names the precise rule: $(field .detail)" ;;
    "") bad "  the answer has no reason" ;;
    *) ok "  with a reason for a person: $(field .detail)" ;;
  esac
}
refused "a host the allow list does not name" "https://example.com/mcp"
refused "the cloud's metadata address" "http://169.254.169.254/latest"
refused "a URL with user information" "http://user:pw@$host:8080/mcp"
refused "a URL with a query" "http://$host:8080/mcp?key=1"

# --- another person ---------------------------------------------------------------------------------------------
expect "tool-maker-2: GET $api" "$(call "$h_maker2" GET "$api")" 200
expect "  lists none of the first's" "$(field '.servers | length')" 0
call "$h_maker2" GET /api/tool-servers >/dev/null
expect "  and the picker has only the deployment's" "$(field 'length')" 1
expect "tool-maker-2: PATCH an id of the first's" "$(call "$h_maker2" PATCH "$api/$bearer_id" '{"name":"mine"}')" 404
other_patch=$(field .detail)
expect "tool-maker-2: DELETE an id of the first's" "$(call "$h_maker2" DELETE "$api/$bearer_id")" 404
expect "tool-maker-2: PATCH an id that never existed" "$(call "$h_maker2" PATCH "$api/u-0000000000" '{"name":"mine"}')" 404
expect "  is the same answer" "$(field .detail)" "$other_patch"
expect "someone-else (no permission): PATCH is refused for the permission" "$(call "$h_else" PATCH "$api/$bearer_id" '{"name":"mine"}')" 403
expect "someone-else: DELETE of an id of the first's" "$(call "$h_else" DELETE "$api/$bearer_id")" 404
expect "dev: DELETE of an id of the first's" "$(call "$h_dev" DELETE "$api/$key_id")" 404
expect "the first's servers are as they were" "$(call "$h_maker" GET "$api")" 200
expect "  three" "$(field '.servers | length')" 3

# --- at rest, in the logs ------------------------------------------------------------------------------------------
compose() { docker compose -f "$root/compose.yaml" --profile app "$@"; }
if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1 && compose ps -q postgres 2>/dev/null | grep -q .; then
  sql() { compose exec -T postgres psql -U postgres -d orch -At -c "$1" 2>/dev/null; }
  sealed=$(sql "select count(*) from user_secrets" || echo unreadable)
  case $sealed in
    '' | unreadable) bad "the table user_secrets cannot be read from the compose postgres" ;;
    *) if [ "$sealed" -ge 2 ]; then ok "AT REST: user_secrets holds $sealed sealed rows (a credential each of the bearer and the API key servers)"; else bad "AT REST: user_secrets holds $sealed rows, want at least 2"; fi ;;
  esac
  dump=$(sql "select t::text from user_secrets t union all select t::text from user_tool_servers t" || echo "")
  found=0
  for secret in "$bearer1" "$bearer2" "$key1" "$key2"; do
    case $dump in *"$secret"*) found=1 ;; esac
  done
  expect "AT REST: no secret is in user_secrets or user_tool_servers" "$found" 0
  logs=$(compose logs --no-color orchestrator 2>&1 || true)
  found=0
  for secret in "$bearer1" "$bearer2" "$key1" "$key2" "$in_path"; do
    case $logs in *"$secret"*) found=1 ;; esac
  done
  expect "the orchestrator's log has no secret and no URL path" "$found" 0
  case $logs in *"u-"*) ok "  (the log does name the servers, by their ids, so it was read)" ;; *) note "  the log names no server; the check above may have read nothing" ;; esac
else
  note "AT REST and the log: docker compose is not available here, or the stack is not its project; the responses were checked all the same"
fi

# --- the limit ---------------------------------------------------------------------------------------------------
call "$h_maker" GET "$api" >/dev/null
n=$(jq -r '.servers | length' "$tmp/body")
while [ "$n" -lt 8 ]; do
  _s=$(call "$h_maker" POST "$api" "$(add_body "filler $n" "$url" '{"kind":"none"}')")
  [ "$_s" = 201 ] || { bad "adding a server up to the limit: got $_s"; break; }
  n=$((n + 1))
done
expect "the ninth server" "$(call "$h_maker" POST "$api" "$(add_body ninth "$url" '{"kind":"none"}')")" 409
expect "  is too_many_servers" "$(field .code)" too_many_servers
expect "tool-maker-2 has places of their own" "$(call "$h_maker2" POST "$api" "$(add_body "two's" "$url" '{"kind":"none"}')")" 201

# --- deleting them -------------------------------------------------------------------------------------------------
expect "DELETE the bearer server" "$(call "$h_maker" DELETE "$api/$bearer_id")" 204
expect "DELETE it again" "$(call "$h_maker" DELETE "$api/$bearer_id")" 404
expect "DELETE the API key server" "$(call "$h_maker" DELETE "$api/$key_id")" 204
clean
expect "GET $api after" "$(call "$h_maker" GET "$api")" 200
expect "  nothing is left of the first's" "$(field '.servers | length')" 0
if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1 && compose ps -q postgres 2>/dev/null | grep -q .; then
  expect "AT REST: the sealed rows went with their servers" "$(sql "select count(*) from user_secrets" || echo unreadable)" 0
fi

# --- nothing the API said held a secret ------------------------------------------------------------------------------
found=0
for secret in "$bearer1" "$bearer2" "$key1" "$key2"; do
  if grep -qF "$secret" "$tmp/seen"; then found=1; fi
done
expect "no response of this script held a secret" "$found" 0

finish
