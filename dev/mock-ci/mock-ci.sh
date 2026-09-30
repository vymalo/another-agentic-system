#!/bin/sh
# A CI stand-in for the local stack. Every MOCK_CI_POLL_SECS it lists the `agent/*` branches of the
# repositories on the git server (`git ls-remote`), and for each commit it has not reported yet it
# posts one SIGNED check result to the orchestrator, the way a real CI system would (ADR 0017,
# docs/api/webhooks.md):
#
#   MOCK_CI_SHAPE=github   (default) a GitHub `check_run` delivery (completed) to /webhooks/github, named
#                          `mock-ci/build`, signed with X-Hub-Signature-256 over the raw body
#   MOCK_CI_SHAPE=github-workflow  a GitHub `workflow_run` delivery (completed), named `mock-ci/build`
#   MOCK_CI_SHAPE=generic  the generic body to /webhooks/ci, signed over "<timestamp>.<body>"
#
# The check is named `mock-ci/build`: the gate names the checks that count (`gate.ci.required` of the
# coder's entry in dev/agents.yaml), and a report of another name is only a card.
#
# The conclusion is `failure` when the commit's message contains CI_FAIL, `success` otherwise. A
# commit is reported once, and remembered in $MOCK_CI_STATE; a restart that forgot what it reported
# posts it again with a new completion time, which the orchestrator stores as a new report (the idempotency
# key of a delivery is made of what is signed, not of an id the sender chooses).
# A post that is not answered 2xx is tried again on the next pass.
#
# The secret is read from the environment and never appears on a command line (`openssl dgst -hmac KEY`
# would show it to every process of the machine): the HMAC is built from two plain SHA-256 runs.
#
# Environment (defaults match compose.yaml):
#   GIT_SERVER_URL      http://git-server:8080   where `git ls-remote` goes: <url>/<repo>.git
#   MOCK_CI_REPOS       local/sandbox            repositories to watch, space separated
#   MOCK_CI_REPO_URL    <GIT_SERVER_URL>         the address reported as the repository: <it>/<repo>. It must
#                                                name the repository the way the agent's `branch` artifact does
#   WEBHOOK_URL         http://edge:8080         where the orchestrator's webhooks are served (the edge passes
#                                                /webhooks/* on without an identity)
#   WEBHOOK_SECRET      (required)               the shared secret (WEBHOOK_GENERIC_SECRETS / WEBHOOK_GITHUB_SECRETS),
#                                                at least 32 bytes
#   MOCK_CI_SHAPE       github                   github, github-workflow or generic
#   MOCK_CI_POLL_SECS   2
#   MOCK_CI_STATE       /var/lib/mock-ci         what was reported (one empty file per commit)
#   MOCK_CI_ONCE        unset                    1 = one pass, then exit (non-zero if a post failed)
#
# POSIX sh; needs git, curl, openssl, od and awk.
set -eu

git_server=${GIT_SERVER_URL:-http://git-server:8080}
git_server=${git_server%/}
repos=${MOCK_CI_REPOS:-local/sandbox}
repo_url=${MOCK_CI_REPO_URL:-$git_server}
repo_url=${repo_url%/}
webhook=${WEBHOOK_URL:-http://edge:8080}
webhook=${webhook%/}
secret=${WEBHOOK_SECRET:-}
shape=${MOCK_CI_SHAPE:-github}
poll=${MOCK_CI_POLL_SECS:-2}
state=${MOCK_CI_STATE:-/var/lib/mock-ci}
once=${MOCK_CI_ONCE:-}

case $shape in github | github-workflow | generic) ;; *) echo "mock-ci: MOCK_CI_SHAPE must be github, github-workflow or generic, not '$shape'" >&2; exit 2 ;; esac
[ -n "$secret" ] || { echo "mock-ci: WEBHOOK_SECRET is required" >&2; exit 2; }

log() { echo "mock-ci: $*"; }

mkdir -p "$state"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
git init -q --bare "$scratch/repo"

# hexbin: hex on stdin, the same bytes as printf escapes on stdout.
hexbin() {
  awk '{ h = "0123456789abcdef"
         for (i = 1; i < length($0); i += 2) {
           printf "\\0%03o", (index(h, substr($0, i, 1)) - 1) * 16 + index(h, substr($0, i + 1, 1)) - 1 } }'
}

# padxor N: the key (hex on stdin) padded with zeros to one 64-byte block and XORed with byte N.
padxor() {
  awk -v n="$1" '
    function xor(a, b,    r, p, i) { r = 0; p = 1
      for (i = 0; i < 8; i++) { if ((a % 2) != (b % 2)) r += p; a = int(a / 2); b = int(b / 2); p *= 2 }
      return r }
    { h = "0123456789abcdef"; k = $0
      while (length(k) < 128) k = k "0"
      out = ""
      for (i = 1; i < length(k); i += 2) {
        v = xor((index(h, substr(k, i, 1)) - 1) * 16 + index(h, substr(k, i + 1, 1)) - 1, n)
        out = out substr(h, int(v / 16) + 1, 1) substr(h, v % 16 + 1, 1) }
      print out }'
}

# hmac: HMAC-SHA-256 of stdin under $secret, as lowercase hex (RFC 2104 by hand: H((K^opad) || H((K^ipad) || m))).
# Only shell builtins ever see the secret; no command line carries it.
hmac() {
  cat >"$scratch/message"
  keyhex=$(printf '%s' "$secret" | od -An -v -tx1 | tr -d ' \n')
  if [ "${#keyhex}" -gt 128 ]; then keyhex=$(printf '%s' "$secret" | openssl dgst -sha256 -r | cut -d' ' -f1); fi
  ipad=$(printf '%s' "$keyhex" | padxor 54)
  opad=$(printf '%s' "$keyhex" | padxor 92)
  inner=$({ printf '%b' "$(printf '%s' "$ipad" | hexbin)"; cat "$scratch/message"; } |
    openssl dgst -sha256 -binary | od -An -v -tx1 | tr -d ' \n')
  { printf '%b' "$(printf '%s' "$opad" | hexbin)"; printf '%b' "$(printf '%s' "$inner" | hexbin)"; } |
    openssl dgst -sha256 -r | cut -d' ' -f1
}

# number_of TEXT: a positive number derived from TEXT (the first 8 hex digits of its SHA-256).
number_of() {
  h=$(printf '%s' "$1" | openssl dgst -sha256 -r | cut -c1-8)
  echo "$((0x$h + 1))"
}

# post_github REPO BRANCH SHA CONCLUSION SUMMARY: a completed check_run (or workflow_run) named mock-ci/build.
# Both carry the repository and its head repository as the same one, and the time it completed.
post_github() {
  now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  id=$(number_of "$1@$3")
  repo_json=$(printf '{"id":1,"name":"%s","full_name":"%s","html_url":"%s/%s"}' "${1#*/}" "$1" "$repo_url" "$1")
  if [ "$shape" = github-workflow ]; then
    event=workflow_run
    body=$(printf '{"action":"completed","workflow_run":{"id":%s,"name":"mock-ci/build","head_branch":"%s","head_sha":"%s","status":"completed","conclusion":"%s","run_attempt":1,"updated_at":"%s","html_url":"%s/%s/actions/runs/%s","head_repository":%s},"repository":%s}' \
      "$id" "$2" "$3" "$4" "$now" "$repo_url" "$1" "$id" "$repo_json" "$repo_json")
  else
    event=check_run
    body=$(printf '{"action":"completed","check_run":{"id":%s,"name":"mock-ci/build","head_sha":"%s","status":"completed","conclusion":"%s","completed_at":"%s","html_url":"%s/%s/runs/%s","output":{"summary":"%s"},"check_suite":{"head_branch":"%s"},"pull_requests":[]},"repository":%s}' \
      "$id" "$3" "$4" "$now" "$repo_url" "$1" "$id" "$5" "$2" "$repo_json")
  fi
  curl -sS -o /dev/null -w '%{http_code}' --max-time 30 -X POST "$webhook/webhooks/github" \
    -H 'Content-Type: application/json' -H 'User-Agent: mock-ci' \
    -H "X-GitHub-Event: $event" -H "X-GitHub-Delivery: mock-ci-$id" \
    -H "X-Hub-Signature-256: sha256=$(printf '%s' "$body" | hmac)" \
    --data-binary "$body"
}

# post_generic REPO BRANCH SHA CONCLUSION SUMMARY: the generic body.
post_generic() {
  body=$(printf '{"version":1,"repository":"%s/%s","sha":"%s","branch":"%s","name":"mock-ci/build","conclusion":"%s","summary":"%s"}' \
    "$repo_url" "$1" "$3" "$2" "$4" "$5")
  ts=$(date +%s)
  curl -sS -o /dev/null -w '%{http_code}' --max-time 30 -X POST "$webhook/webhooks/ci" \
    -H 'Content-Type: application/json' -H 'User-Agent: mock-ci' \
    -H "X-Vymalo-Timestamp: $ts" \
    -H "X-Vymalo-Signature-256: sha256=$(printf '%s.%s' "$ts" "$body" | hmac)" \
    --data-binary "$body"
}

# report REPO BRANCH SHA: decides, posts, and remembers it once the orchestrator took it.
report() {
  marker=$state/$(printf '%s' "$1@$3" | openssl dgst -sha256 -r | cut -d' ' -f1)
  [ ! -e "$marker" ] || return 0
  if ! git -C "$scratch/repo" fetch -q --depth 1 "$git_server/$1.git" "refs/heads/$2" 2>"$scratch/err"; then
    log "cannot fetch $1 $2: $(head -c 200 "$scratch/err")"
    return 1
  fi
  message=$(git -C "$scratch/repo" log -1 --format=%B FETCH_HEAD)
  case $message in
    *CI_FAIL*) conclusion=failure; summary="CI_FAIL in the commit message" ;;
    *) conclusion=success; summary="the mock CI passed" ;;
  esac
  if [ "$shape" = generic ]; then status=$(post_generic "$1" "$2" "$3" "$conclusion" "$summary") || status=000
  else status=$(post_github "$1" "$2" "$3" "$conclusion" "$summary") || status=000; fi
  case $status in
    2??) : >"$marker"; log "$shape: $conclusion for $1 $2 $(printf '%s' "$3" | cut -c1-7): HTTP $status" ;;
    *) log "$shape: $conclusion for $1 $2 $(printf '%s' "$3" | cut -c1-7): HTTP $status, will retry"; return 1 ;;
  esac
}

pass() {
  failed=0
  for repo in $repos; do
    if ! heads=$(git ls-remote --heads "$git_server/$repo.git" 'refs/heads/agent/*' 2>"$scratch/err"); then
      log "cannot list $repo: $(head -c 200 "$scratch/err")"
      failed=1
      continue
    fi
    [ -n "$heads" ] || continue
    # One "<sha> <tab> refs/heads/<branch>" per line.
    printf '%s\n' "$heads" | while IFS='	' read -r sha ref; do
      branch=${ref#refs/heads/}
      # The branch name goes into JSON unescaped: only plain names are reported.
      case $branch in *[!A-Za-z0-9._/-]* | '') log "skipping the branch '$branch': not a plain name"; continue ;; esac
      report "$repo" "$branch" "$sha" || echo failed >"$scratch/failed"
    done
    if [ -e "$scratch/failed" ]; then rm -f "$scratch/failed"; failed=1; fi
  done
  return "$failed"
}

log "watching $repos on $git_server, reporting $shape results to $webhook"
while :; do
  if pass; then ok=0; else ok=1; fi
  [ -z "$once" ] || exit "$ok"
  sleep "$poll"
done
