#!/usr/bin/env sh
# System-level test of who may do what (ADR 0033, plan 10 section 3.4, and ADR 0039: nobody reads another person's thread): the stack signs people in at a mock issuer behind a real
# oauth2-proxy, the orchestrator is an OAuth2 resource server (`auth.mode: jwt`), and the roles of the token (dev/mock-oidc/users.json)
# decide what each person may do (`auth.roles` of dev/orchestrator.yaml).
#
#   dev/rbac-e2e.sh
#
# Start the `app` profile first (docker compose --profile app up -d --build --wait). The script needs only the `chat` agent (it
# answers on a scripted model, nothing is pushed): it starts one thread as each of `dev@example.com`, `chat-only@example.com` and `admin@example.com`, then
# asserts, one ok or FAIL line each:
#   * GET /api/me, for each of the four users: the user, the roles that count, what they grant and the agents they are about
#     (dev: user; admin: user plus `admin`, which is operational and content-free, so thread.read, thread.write and artifact.read are
#     `own` for it as for everyone (ADR 0039); chat-only: the agent `chat` alone; guest: no role in the token, so the default role,
#     user), in the shape of docs/api/chat-api.yaml (`Me`);
#   * a token for another audience (the issuer signs it, oauth2-proxy and the orchestrator refuse it), no token, a bad token and
#     only the old identity header are all 401;
#   * a user sees only their own threads (GET /api/threads), and so does the administrator; asking for another's or everyone's
#     (`?owner=`, any value) is a 400 for both, for nobody lists another person's threads (ADR 0039);
#   * the administrator is a user over their own thread (they start one, read it, export it, rename it) and gets 404, as for a thread
#     that does not exist, for every read (the thread, its export, its branches, its AG-UI stream) and every act (a message through
#     AG-UI, a rename, a cancel, a fork) on another person's thread, which is left as it was; a user does too;
#   * `chat-only` gets 403 `forbidden` for POST /agui/agents/coder, 200 for chat, and GET /api/agents lists only `chat`.
# Not staged: an issuer that is down at startup (the orchestrator then answers 503 with Retry-After and /readyz is 503). It needs
# the issuer stopped while the orchestrator restarts, which would leave the stack broken if this script were killed half way; the
# Rust test `in_jwt_mode_only_a_valid_token_is_an_identity_and_readiness_follows_the_keys` (orchestrator/bin/orchestrator/tests/smoke.rs)
# runs the real binary against an issuer that is not there.
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL   http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   OIDC_URL   http://127.0.0.1:${MOCK_OIDC_PORT:-8099}, the mock issuer (dev/auth-header.sh reads it, and OIDC_CLIENT_ID and _SECRET)
#   TIMEOUT    120    seconds to wait for a thread to stop
#
# It leaves three threads behind (one each of dev@example.com, chat-only@example.com and admin@example.com), like any other scenario.
# Needs curl and jq (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
timeout=${TIMEOUT:-120}
here=$(cd "$(dirname "$0")" && pwd)

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "rbac e2e passed"; else echo "rbac e2e FAILED"; exit 1; fi
}
# expect WHAT GOT WANT: one line, ok when they are equal.
expect() {
  if [ "$2" = "$3" ]; then ok "$1: $3"; else bad "$1: got '$2', want '$3'"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

dev=dev@example.com
admin=admin@example.com
chat_only=chat-only@example.com
guest=guest@example.com

# The header line of each user: a token of the mock issuer (dev/auth-header.sh).
header_of() { sh "$here/auth-header.sh" "$1"; }
no_token() { bad "a token for $1 from the mock issuer (is mock-oidc up? OIDC_URL=${OIDC_URL:-http://127.0.0.1:${MOCK_OIDC_PORT:-8099}})"; finish; }
h_dev=$(header_of "$dev") || no_token "$dev"
h_admin=$(header_of "$admin") || no_token "$admin"
h_chat_only=$(header_of "$chat_only") || no_token "$chat_only"
h_guest=$(header_of "$guest") || no_token "$guest"
ok "the mock issuer gave a token to each of the four users"

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

# call HEADER METHOD PATH [JSON]: the status on stdout, the body in $tmp/body (HEADER may be empty: no credential at all).
call() {
  if [ -n "$1" ]; then _auth=$1; else _auth='X-No-Credential: none'; fi
  if [ $# -ge 4 ]; then
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$2" "$base$3" -H "$_auth" -H 'content-type: application/json' -d "$4" 2>"$tmp/err" || true
  else
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$2" "$base$3" -H "$_auth" 2>"$tmp/err" || true
  fi
}

# run HEADER AGENT THREAD TEXT: one AG-UI run (POST /agui/agents/{agent}); the status on stdout, the body in $tmp/body.
run() {
  _input=$(jq -n --arg thread "$3" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$4" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
  curl -sS -N --max-time "$timeout" -o "$tmp/body" -w '%{http_code}' -X POST "$base/agui/agents/$2" -H "$1" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$_input" 2>"$tmp/err" || true
}

# wait_stopped HEADER THREAD: waits for the thread to stop moving.
wait_stopped() {
  _deadline=$(( $(date +%s) + timeout ))
  while :; do
    _state=$(curl -sS --max-time 30 -H "$1" "$base/api/threads/$2" 2>/dev/null | jq -r '.state // empty' || true)
    case $_state in done | blocked | failed | cancelled) break ;; esac
    [ "$(date +%s)" -lt "$_deadline" ] || break
    sleep 1
  done
  echo "${_state:-unknown}"
}

code_of() { jq -r '.code // empty' "$tmp/body" 2>/dev/null || true; }

# --- GET /api/me ------------------------------------------------------------------------------------------
# `me_shape`: what a client needs of /api/me, compactly: user, roles, "permission" or "permission:scope" in the order of the
# answer, and the agents each agent permission is about.
me_shape() {
  jq -cS '{user, roles, permissions: [.permissions[] | .permission + (if .scope then ":" + .scope else "" end)], agents}' "$tmp/body" 2>/dev/null || echo unreadable
}
all='["*"]'
check_me() { # check_me WHO HEADER EMAIL ROLES PERMISSIONS READ INVOKE
  _status=$(call "$2" GET /api/me)
  expect "GET /api/me as $1: status" "$_status" 200
  _want=$(jq -cnS --arg user "$3" --argjson roles "$4" --argjson perms "$5" --argjson read "$6" --argjson invoke "$7" \
    '{user: $user, roles: $roles, permissions: $perms, agents: {read: $read, invoke: $invoke}}')
  expect "GET /api/me as $1 says" "$(me_shape)" "$_want"
}
own='["agent.read","agent.invoke","thread.read:own","thread.write:own","artifact.read:own"]'
check_me "dev (a user)" "$h_dev" "$dev" '["user"]' "$own" "$all" "$all"
check_me "admin (a user who also holds admin: own threads only, ADR 0039)" "$h_admin" "$admin" '["admin"]' \
  '["agent.read","agent.invoke","thread.read:own","thread.write:own","artifact.read:own","admin"]' "$all" "$all"
check_me "chat-only (the agent chat alone)" "$h_chat_only" "$chat_only" '["chat-only"]' "$own" '["chat"]' '["chat"]'
check_me "guest (no role in the token: the default role)" "$h_guest" "$guest" '["user"]' "$own" "$all" "$all"
_status=$(call "$h_admin" GET /api/me)
expect "GET /api/me names the person the token names" "$(jq -r '[.email, .name] | join(" / ")' "$tmp/body")" "$admin / Admin User"

# --- what is not a token, or not for this API: 401 -----------------------------------------------------------
expect "GET /api/me with no credential" "$(call '' GET /api/me)" 401
expect "GET /api/me with a bearer that is not a token" "$(call 'Authorization: Bearer not.a.token' GET /api/me)" 401
expect "GET /api/me with only the old identity header (a client's X-Auth-Request-Email is no identity)" \
  "$(call "X-Auth-Request-Email: $admin" GET /api/me)" 401
wrong=$(OIDC_AUDIENCE=someone-else sh "$here/auth-header.sh" "$dev" || true)
if [ -n "$wrong" ]; then
  expect "GET /api/me with a token signed by the issuer for another audience" "$(call "$wrong" GET /api/me)" 401
  expect "POST /agui/agents/chat with that token" "$(run "$wrong" chat "$(uuid)" hi)" 401
else
  bad "a token for another audience from the mock issuer (OIDC_AUDIENCE=someone-else)"
fi

# --- one thread each, on the agent every role may use ---------------------------------------------------------
dev_thread=$(uuid)
chat_thread=$(uuid)
admin_thread=$(uuid)
expect "dev starts a thread on chat" "$(run "$h_dev" chat "$dev_thread" hi)" 200
expect "dev's thread ends" "$(wait_stopped "$h_dev" "$dev_thread")" "done"
expect "chat-only starts a thread on chat" "$(run "$h_chat_only" chat "$chat_thread" hi)" 200
expect "chat-only's thread ends" "$(wait_stopped "$h_chat_only" "$chat_thread")" "done"
expect "admin starts a thread on chat" "$(run "$h_admin" chat "$admin_thread" hi)" 200
expect "admin's thread ends" "$(wait_stopped "$h_admin" "$admin_thread")" "done"

# --- a user sees only their own threads ----------------------------------------------------------------------
_status=$(call "$h_dev" GET '/api/threads?limit=100')
expect "GET /api/threads as dev: status" "$_status" 200
expect "dev's list holds dev's thread" "$(jq -r --arg t "$dev_thread" '[.[] | select(.id == $t)] | length' "$tmp/body")" 1
expect "dev's list holds no thread of chat-only" "$(jq -r --arg t "$chat_thread" '[.[] | select(.id == $t)] | length' "$tmp/body")" 0
expect "every thread of dev's list is dev's" "$(jq -r --arg u "$dev" '[.[] | select(.owner != $u)] | length' "$tmp/body")" 0
_status=$(call "$h_chat_only" GET '/api/threads?limit=100')
expect "chat-only's list holds chat-only's thread and not dev's" \
  "$(jq -r --arg a "$chat_thread" --arg b "$dev_thread" '[.[] | select(.id == $a)] | length, ([.[] | select(.id == $b)] | length)' "$tmp/body" | tr '\n' ' ')" "1 0 "
# `?owner=` is gone for everyone, naming oneself included: a 400 that says so, never a quiet list (ADR 0039).
for who in dev admin; do
  case $who in dev) _h=$h_dev ;; *) _h=$h_admin ;; esac
  for q in 'owner=*' "owner=$chat_only" "owner=$dev" "owner=$admin"; do
    expect "$who asks GET /api/threads?$q: refused" "$(call "$_h" GET "/api/threads?$q")" 400
  done
  expect "  the reason" "$(jq -r .detail "$tmp/body")" "owner is not supported (ADR 0039)"
done

# --- the administrator is a user over their own threads, and reads nobody else's ---------------------------------
_status=$(call "$h_admin" GET '/api/threads?limit=100')
expect "GET /api/threads as admin: status" "$_status" 200
expect "admin's list holds admin's thread and neither dev's nor chat-only's" \
  "$(jq -r --arg a "$admin_thread" --arg b "$dev_thread" --arg c "$chat_thread" '[.[] | select(.id == $a)] | length, ([.[] | select(.id == $b or .id == $c)] | length)' "$tmp/body" | tr '\n' ' ')" "1 0 "
expect "every thread of admin's list is admin's" "$(jq -r --arg u "$admin" '[.[] | select(.owner != $u)] | length' "$tmp/body")" 0
expect "admin reads their own thread" "$(call "$h_admin" GET "/api/threads/$admin_thread")" 200
expect "  the thread says its owner" "$(jq -r .owner "$tmp/body")" "$admin"
expect "admin exports their own thread" "$(call "$h_admin" GET "/api/threads/$admin_thread/export")" 200
expect "admin renames their own thread" "$(call "$h_admin" PATCH "/api/threads/$admin_thread" '{"title": "mine"}')" 200

# Another person's thread does not exist for the administrator: 404 on every read and every act, as for a user.
_status=$(call "$h_dev" GET "/api/threads/$dev_thread")
dev_seq=$(jq -r '.lastSeq' "$tmp/body")
for t in "$dev_thread" "$chat_thread" "$(uuid)"; do
  expect "admin reads thread $t (GET /api/threads/{id})" "$(call "$h_admin" GET "/api/threads/$t")" 404
done
expect "admin exports dev's thread" "$(call "$h_admin" GET "/api/threads/$dev_thread/export")" 404
expect "admin reads dev's branches" "$(call "$h_admin" GET "/api/threads/$dev_thread/branches")" 404
expect "admin follows dev's thread over AG-UI (GET /agui/threads/{id}/connect)" \
  "$(curl -sS --max-time 30 -o "$tmp/body" -w '%{http_code}' -H "$h_admin" -H 'accept: text/event-stream' "$base/agui/threads/$dev_thread/connect" 2>"$tmp/err" || true)" 404
expect "admin sends dev's thread a message (POST /agui/agents/chat): refused" "$(run "$h_admin" chat "$dev_thread" "hello from the administrator")" 404
expect "admin renames dev's thread (PATCH): refused" "$(call "$h_admin" PATCH "/api/threads/$dev_thread" '{"title": "taken over"}')" 404
expect "admin cancels dev's thread: refused" "$(call "$h_admin" POST "/api/threads/$dev_thread/cancel")" 404
expect "admin forks dev's thread: refused" "$(call "$h_admin" POST "/api/threads/$dev_thread/fork" '{"after": 1}')" 404
_status=$(call "$h_dev" GET "/api/threads/$dev_thread")
expect "dev's thread was left as it was (still done, not retitled)" "$(jq -r '[.state, (.title == "taken over")] | join(" ")' "$tmp/body")" "done false"
expect "  and its log is as long as before" "$(jq -r '.lastSeq' "$tmp/body")" "$dev_seq"

# --- a user reading another user's thread: 404, as for a thread that does not exist ----------------------------
expect "chat-only reads dev's thread" "$(call "$h_chat_only" GET "/api/threads/$dev_thread")" 404
expect "dev reads admin's thread" "$(call "$h_dev" GET "/api/threads/$admin_thread")" 404
expect "dev reads chat-only's thread" "$(call "$h_dev" GET "/api/threads/$chat_thread")" 404
expect "dev reads a thread that does not exist" "$(call "$h_dev" GET "/api/threads/$(uuid)")" 404

# --- chat-only: the agent chat and no other ------------------------------------------------------------------
expect "chat-only starts a thread on coder (POST /agui/agents/coder): refused" "$(run "$h_chat_only" coder "$(uuid)" hi)" 403
expect "  the code" "$(code_of)" forbidden
expect "chat-only starts a thread on researcher: refused" "$(run "$h_chat_only" researcher "$(uuid)" hi)" 403
expect "chat-only goes on in its own thread on chat" "$(run "$h_chat_only" chat "$chat_thread" "hi again")" 200
_status=$(call "$h_chat_only" GET /api/agents)
expect "GET /api/agents as chat-only: status" "$_status" 200
expect "chat-only is listed the agent chat and no other" "$(jq -r '[.[].id] | join(" ")' "$tmp/body")" chat
_status=$(call "$h_dev" GET /api/agents)
expect "GET /api/agents as dev lists the coder, the chat and the researcher" \
  "$(jq -r '[.[].id] | map(select(. == "coder" or . == "chat" or . == "researcher")) | join(" ")' "$tmp/body")" "coder chat researcher"
finish
