#!/bin/sh
# A CI stand-in for the local stack. Every MOCK_CI_POLL_SECS it lists the `agent/*` branches of the
# repositories on the git server (`git ls-remote`), and for each commit it has not reported yet it
# posts one SIGNED check result to the orchestrator, the way a real CI system would (ADR 0017,
# docs/api/webhooks.md):
#
#   MOCK_CI_SHAPE=github   (default) a GitHub `check_suite` delivery (completed) to /webhooks/github,
#                          signed with X-Hub-Signature-256 over the raw body
#   MOCK_CI_SHAPE=generic  the generic body to /webhooks/ci, signed over "<timestamp>.<body>"
#
# The conclusion is `failure` when the commit's message contains CI_FAIL, `success` otherwise. A
# commit is reported once: the delivery id is derived from the repository and the commit, so a
# restart that forgets what it reported sends the same id, which the orchestrator takes as a repeat.
# A post that is not answered 2xx is tried again on the next pass.
#
# Environment (defaults match compose.yaml):
#   GIT_SERVER_URL      http://git-server:8080   where `git ls-remote` goes: <url>/<repo>.git
#   MOCK_CI_REPOS       local/sandbox            repositories to watch, space separated
#   MOCK_CI_REPO_URL    <GIT_SERVER_URL>         the address reported as the repository: <it>/<repo>. It must
#                                                name the repository the way the agent's `branch` artifact does
#   WEBHOOK_URL         http://edge:8080         where the orchestrator's webhooks are served (the edge passes
#                                                /webhooks/* on without an identity)
#   WEBHOOK_SECRET      dev-webhook-secret       the shared secret (WEBHOOK_GENERIC_SECRETS / WEBHOOK_GITHUB_SECRETS)
#   MOCK_CI_SHAPE       github                   github or generic
#   MOCK_CI_POLL_SECS   2
#   MOCK_CI_STATE       /var/lib/mock-ci         what was reported (one empty file per commit)
#   MOCK_CI_ONCE        unset                    1 = one pass, then exit (non-zero if a post failed)
#
# POSIX sh; needs git, curl and openssl.
set -eu

git_server=${GIT_SERVER_URL:-http://git-server:8080}
git_server=${git_server%/}
repos=${MOCK_CI_REPOS:-local/sandbox}
repo_url=${MOCK_CI_REPO_URL:-$git_server}
repo_url=${repo_url%/}
webhook=${WEBHOOK_URL:-http://edge:8080}
webhook=${webhook%/}
secret=${WEBHOOK_SECRET:-dev-webhook-secret}
shape=${MOCK_CI_SHAPE:-github}
poll=${MOCK_CI_POLL_SECS:-2}
state=${MOCK_CI_STATE:-/var/lib/mock-ci}
once=${MOCK_CI_ONCE:-}

case $shape in github | generic) ;; *) echo "mock-ci: MOCK_CI_SHAPE must be github or generic, not '$shape'" >&2; exit 2 ;; esac

log() { echo "mock-ci: $*"; }

mkdir -p "$state"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
git init -q --bare "$scratch/repo"

# hmac: HMAC-SHA-256 of stdin under $secret, as lowercase hex.
hmac() { openssl dgst -sha256 -hmac "$secret" -r | cut -d' ' -f1; }

# uuid_of TEXT: a UUID derived from TEXT (8-4-4-4-12 of its SHA-256).
uuid_of() {
  h=$(printf '%s' "$1" | openssl dgst -sha256 -r | cut -c1-32)
  printf '%s-%s-%s-%s-%s' "$(echo "$h" | cut -c1-8)" "$(echo "$h" | cut -c9-12)" "$(echo "$h" | cut -c13-16)" \
    "$(echo "$h" | cut -c17-20)" "$(echo "$h" | cut -c21-32)"
}

# post_github REPO BRANCH SHA CONCLUSION DELIVERY: a completed check_suite.
post_github() {
  body=$(printf '{"action":"completed","check_suite":{"id":%s,"head_branch":"%s","head_sha":"%s","status":"completed","conclusion":"%s","app":{"slug":"mock-ci","name":"Mock CI"}},"repository":{"full_name":"%s","html_url":"%s/%s"}}' \
    "$(($(echo "$5" | cut -c1-8 | sed 's/^/0x/')))" "$2" "$3" "$4" "$1" "$repo_url" "$1")
  curl -sS -o /dev/null -w '%{http_code}' --max-time 30 -X POST "$webhook/webhooks/github" \
    -H 'Content-Type: application/json' -H 'User-Agent: mock-ci' \
    -H 'X-GitHub-Event: check_suite' -H "X-GitHub-Delivery: $5" \
    -H "X-Hub-Signature-256: sha256=$(printf '%s' "$body" | hmac)" \
    --data-binary "$body"
}

# post_generic REPO BRANCH SHA CONCLUSION DELIVERY SUMMARY: the generic body.
post_generic() {
  body=$(printf '{"version":1,"repository":"%s/%s","sha":"%s","branch":"%s","name":"mock-ci/build","conclusion":"%s","summary":"%s"}' \
    "$repo_url" "$1" "$3" "$2" "$4" "$6")
  ts=$(date +%s)
  curl -sS -o /dev/null -w '%{http_code}' --max-time 30 -X POST "$webhook/webhooks/ci" \
    -H 'Content-Type: application/json' -H 'User-Agent: mock-ci' \
    -H "X-Vymalo-Delivery: $5" -H "X-Vymalo-Timestamp: $ts" \
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
  id=$(uuid_of "$1@$3")
  if [ "$shape" = github ]; then status=$(post_github "$1" "$2" "$3" "$conclusion" "$id") || status=000
  else status=$(post_generic "$1" "$2" "$3" "$conclusion" "$id" "$summary") || status=000; fi
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
