#!/usr/bin/env sh
# System-level test of the default agent: one chat message becomes a pull request.
#
#   dev/coder-e2e.sh                  # OpenCode (model mock-opencode) makes the change
#   NO_OPENCODE=1 dev/coder-e2e.sh    # the check command makes it; OpenCode is not started
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the UI does (docs/api/agui.md): it runs a thread for the default agent
# (the first entry of dev/agents.yaml) with one POST /agui/agents/{agentId} whose message names the
# seeded repository, waits for a terminal state, and checks the whole chain. The agent list and the
# thread state come from the resource API. (The legacy chat API routes were removed on 2026-09-30.)
# Before the task it puts an empty commit naming the thread on main of the sandbox, so that the pushed
# commit is this run's own even when an earlier run made the same change in the same second.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * the default agent of GET /api/agents is `coder`;
#   * the run stream ends with RUN_FINISHED (success), and the thread ends `done` within TIMEOUT;
#   * the thread's AG-UI frames (GET /agui/threads/{id}/connect?mode=run) carry the `checks`, `branch` and
#     `pull_request` artifacts (`vymalo.artifact` activities; their JSON is in `content.text`). Since adam-rs
#     ae540e9 the coder emits `checks` twice, in this order: from `run_checks` (bound to the HEAD it ran on)
#     and from `commit_and_push` (bound to the pushed commit, before `branch`). The LAST one must have
#     passed on exactly the pushed commit, with a 40-hex tree;
#   * the coder is gated on its own checks AND on CI (dev/agents.yaml, ADR 0017, ADR 0018): the thread's job has
#     the gate `ci+agent_checks` (the order of the sources), the chat shows an `agent_checks` `vymalo.check`
#     card that passed on the pushed commit, and exactly one `vymalo.ci` card, the check `mock-ci/build`,
#     `success`, for the pushed commit (the thread ends `done` only after mock-ci reported it through the edge
#     and the orchestrator's GitHub webhook). When the thread does not end `done`, the last lines of the
#     orchestrator's and mock-ci's logs are printed (the watch key of the pushed commit and of the report,
#     for a mismatch of repository spelling or commit);
#   * mock-github saw exactly one POST /repos/local/sandbox/pulls, head = the branch, base = main;
#   * mock-openai matched every request, and saw mock-opencode requests unless NO_OPENCODE=1;
#   * git-server has the branch, and hello.txt on it is `hello`.
# The models are the scripts vendored in dev/coder/wiremock/mock-openai (see dev/coder/UPSTREAM);
# [mock:no-opencode] in the task selects the variant without OpenCode.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`, which injects the identity
#   AUTH_EMAIL       dev@example.com, sent as X-Auth-Request-Email (the edge replaces it; it matters
#                    only when BASE_URL is an orchestrator without the edge)
#   MOCK_GITHUB_URL  http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}
#   GIT_SERVER_URL   http://127.0.0.1:${GIT_SERVER_PORT:-8093}   (from the host)
#   TIMEOUT          300    seconds to wait for the thread to end
#   NO_OPENCODE      unset  1 = the [mock:no-opencode] script
#
# Needs curl, jq and git (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
github=${MOCK_GITHUB_URL:-http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}}
github=${github%/}
openai=${MOCK_OPENAI_URL:-http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}}
openai=${openai%/}
gitserver=${GIT_SERVER_URL:-http://127.0.0.1:${GIT_SERVER_PORT:-8093}}
gitserver=${gitserver%/}
timeout=${TIMEOUT:-300}
repo_path=local/sandbox
# The address the coder, inside the compose network, uses for the repository.
repo_url=http://git-server:8080/$repo_path.git

root=$(cd "$(dirname "$0")/.." && pwd)

# dump_logs: the tail of the logs that say why a CI report did not reach its job, when docker is here.
dump_logs() {
  command -v docker >/dev/null 2>&1 || { echo "     (docker is not available: read the orchestrator and mock-ci logs by hand)"; return 0; }
  for service in orchestrator mock-ci; do
    echo "     --- docker compose logs --tail 60 $service"
    docker compose -f "$root/compose.yaml" --profile app logs --no-color --tail 60 "$service" 2>&1 | sed 's/^/     /' || true
  done
}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "coder e2e passed"; else echo "coder e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "X-Auth-Request-Email: $email"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, one JSON per line
  sed -n 's/^data: *//p' "$1"
}

# --- the default agent -----------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  default_agent=$(printf '%s' "$agents" | jq -r '.[0].id // empty')
  if [ "$default_agent" = coder ]; then
    ok "the default agent (first of /api/agents) is coder"
  else
    bad "the default agent is '${default_agent:-none}', want coder (agents: $(printf '%s' "$agents" | jq -c '[.[].id]'))"
  fi
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi
# Target whatever the first entry is, as the UI does; the check above says whether that is the coder.
agent_id=${default_agent:-coder}

text="In $repo_url (base branch main), add hello.txt containing hello."
if [ "${NO_OPENCODE:-}" = 1 ]; then
  text="$text [mock:no-opencode]"
  echo "variant: no OpenCode ([mock:no-opencode])"
else
  echo "variant: OpenCode makes the change"
fi

# --- reset both journals, then send the task ------------------------------------------
for m in "$github" "$openai"; do
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$m/__admin/requests" || true)
  if [ "$code" = 200 ]; then ok "journal reset: $m"; else bad "journal reset: $m answered HTTP $code"; fi
done

# The consumer mints the thread id (a UUID); the first run creates the thread, owned by the edge
# identity and targeting the agent of the URL. The response streams until the run ends.
thread=$(uuid)

# --- a base of this run's own -------------------------------------------------------------------------
# The change is always the same (hello.txt on main), and git dates have one-second resolution: two runs in
# the same second (dev/e2e-all.sh runs this script twice in a row) would push the very same commit, which
# is watched by the first job that pushed it, so the second job would never hear its CI report. An empty
# commit naming the thread on main makes this run's commit its own; the tree, and so check.sh, is unchanged.
if git clone -q --depth 1 --branch main "$gitserver/$repo_path.git" "$tmp/base" 2>"$tmp/base.err" &&
  git -C "$tmp/base" -c user.name=coder-e2e -c user.email=coder-e2e@example.invalid -c commit.gpgsign=false \
    commit -q --allow-empty -m "coder-e2e base for thread $thread" 2>>"$tmp/base.err" &&
  git -C "$tmp/base" push -q origin HEAD:main 2>>"$tmp/base.err"; then
  ok "main of $repo_path is at a base of this run's own ($(git -C "$tmp/base" rev-parse --short=10 HEAD))"
else
  bad "cannot give this run a base of its own on main: $(head -c 300 "$tmp/base.err")"
  finish
fi
input=$(jq -n --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$text" '{
  threadId: $thread, runId: $run, state: {}, tools: [], context: [],
  messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}')
echo "thread $thread"
deadline=$(( $(date +%s) + timeout ))
code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
  "$base/agui/agents/$agent_id" -H "X-Auth-Request-Email: $email" \
  -H 'content-type: application/json' -H 'accept: text/event-stream' -d "$input" 2>"$tmp/err" || true)
if [ "$code" != 200 ]; then
  bad "POST /agui/agents/$agent_id answered HTTP ${code:-none}: $(head -c 300 "$tmp/err") $(head -c 300 "$tmp/run.sse")"
  finish
fi
outcome=$(sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
  | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true)
if [ "$outcome" = success ]; then
  ok "the run stream ended with RUN_FINISHED (success)"
else
  bad "the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"
fi

# --- wait for a terminal state ----------------------------------------------------------
# `blocked` is not final for a thread but nothing here will answer it, so it ends the wait too.
state=
while :; do
  state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
  case $state in done | blocked | failed | cancelled) break ;; esac
  if [ "$(date +%s)" -ge "$deadline" ]; then break; fi
  sleep 2
done
# The thread's AG-UI frames as one JSON array: the viewer replay, which closes after the run.
events=$tmp/events.json
curl -sS --max-time 60 -H "X-Auth-Request-Email: $email" -H 'accept: text/event-stream' \
  "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
  echo '[]' > "$events"

if [ "$state" = "done" ]; then
  ok "the thread ended done"
else
  bad "the thread did not end done (state: '${state:-unknown}' after at most ${timeout}s)"
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$events" | head -n 20
  dump_logs
fi

# --- artifacts ---------------------------------------------------------------------------
# An artifact reaches AG-UI as an ACTIVITY_SNAPSHOT of type vymalo.artifact whose content is
# {name, mimeType, text}: the JSON the coder sent as a data part is in `content.text`.
artifact() { # artifact NAME FIELD -> the field of the last artifact of that name ("" if absent)
  jq -r --arg n "$1" --arg f "$2" '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact" and .content.name == $n)
    | .content.text | fromjson? | .[$f] // empty] | last // empty' "$events"
}
checks_passed=$(artifact checks passed)
checks_commit=$(artifact checks commit)
checks_tree=$(artifact checks tree)
checks_count=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact" and .content.name == "checks")] | length' "$events" 2>/dev/null || echo '?')
branch=$(artifact branch branch)
commit=$(artifact branch commit)
pr_url=$(artifact pull_request url)
pr_branch=$(artifact pull_request branch)
# The coder reports its checks (run_checks), then again bound to the pushed commit (commit_and_push).
case $checks_count in
  '' | '?' | 0 | 1) bad "the chat shows $checks_count checks artifacts, want at least 2 (run_checks, then commit_and_push)" ;;
  *) ok "$checks_count checks artifacts (run_checks, then the one bound to the pushed commit)" ;;
esac
if [ "$checks_passed" = true ]; then ok "the last checks artifact passed"; else bad "the last checks artifact: passed is '${checks_passed:-absent}', want true"; fi
if [ -n "$commit" ] && [ "$checks_commit" = "$commit" ]; then
  ok "the last checks artifact is bound to the pushed commit $(printf '%s' "$commit" | cut -c1-10)"
else
  bad "the last checks artifact commit '$checks_commit' is not the pushed commit '$commit'"
fi
if printf '%s' "$checks_tree" | grep -Eq '^[0-9a-f]{40}$'; then ok "the checks artifact names the tree $(printf '%s' "$checks_tree" | cut -c1-10)"; else bad "the checks artifact tree '$checks_tree' is not a 40-hex tree id"; fi
if [ -n "$branch" ] && [ -n "$commit" ]; then
  ok "branch artifact: $branch at $(printf '%s' "$commit" | cut -c1-10)"
else
  bad "no branch artifact with a branch and a commit"
fi
if [ -n "$pr_url" ]; then ok "pull_request artifact: $pr_url"; else bad "no pull_request artifact with a url"; fi
if [ -n "$branch" ] && [ "$pr_branch" = "$branch" ]; then
  ok "the pull request is for the pushed branch"
else
  bad "the pull_request branch '$pr_branch' is not the pushed branch '$branch'"
fi

# --- the gate: the coder's checks and CI (ADR 0017, ADR 0018) -------------------------------------------
# mock-ci polls git-server and posts a signed check_run (mock-ci/build) for the pushed commit; the gate
# waited for it, and the gate names that check (`ci.required` of the coder's entry).
gate=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '(.job.gate // []) | join("+")' || true)
if [ "$gate" = ci+agent_checks ]; then ok "the job runs under the gates ci and agent_checks"; else bad "the job's gate is '${gate:-none}', want ci+agent_checks"; fi
# The coder's own checks, as the gate saw them (source `agent_checks`), in the chat as a vymalo.check card.
check_cards=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.check" and .content.source == "agent_checks")
  | "\(.content.attempt)=\(.content.status)@\(.content.commit)"] | last // empty' "$events" 2>/dev/null || true)
if [ -n "$commit" ] && [ "$check_cards" = "1=passed@$commit" ]; then
  ok "the agent_checks card: passed at attempt 1 on the pushed commit"
else
  bad "the last agent_checks vymalo.check card is '${check_cards:-none}', want 1=passed@${commit:-<commit>}"
fi
ci_cards=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ci")
  | "\(.content.name)=\(.content.conclusion)@\(.content.sha)"] | join(" ")' "$events" 2>/dev/null || true)
if [ -n "$commit" ] && [ "$ci_cards" = "mock-ci/build=success@$commit" ]; then
  ok "one vymalo.ci card: mock-ci/build succeeded on the pushed commit"
else
  bad "the vymalo.ci cards are '${ci_cards:-none}', want exactly mock-ci/build=success@${commit:-<commit>}"
  dump_logs
fi

# --- mock-github's journal ------------------------------------------------------------------
found=$tmp/found.json
curl -s --max-time 30 -X POST "$github/__admin/requests/find" -H 'Content-Type: application/json' \
  -d "{\"method\":\"POST\",\"urlPath\":\"/repos/$repo_path/pulls\"}" > "$found" || true
posts=$(jq -r '.requests | length' "$found" 2>/dev/null || echo '?')
if [ "$posts" = 1 ]; then
  ok "mock-github saw exactly one POST /repos/$repo_path/pulls"
else
  bad "mock-github saw $posts POST /repos/$repo_path/pulls, want exactly 1"
fi
head_ref=$(jq -r '.requests[0].body | fromjson | .head // empty' "$found" 2>/dev/null || true)
base_ref=$(jq -r '.requests[0].body | fromjson | .base // empty' "$found" 2>/dev/null || true)
# A head may be written <owner>:<branch>; only the branch matters.
head_branch=${head_ref##*:}
if [ -n "$branch" ] && [ "$head_branch" = "$branch" ]; then
  ok "the pull request head is $head_branch"
else
  bad "the pull request head is '$head_ref', want '$branch'"
fi
if [ "$base_ref" = main ]; then ok "the pull request base is main"; else bad "the pull request base is '$base_ref', want main"; fi

# --- mock-openai's journal ---------------------------------------------------------------------
# Off-script requests are unmatched (a 404), never a canned answer.
unmatched=$(curl -s --max-time 30 "$openai/__admin/requests/unmatched" | jq -r '.requests | length' 2>/dev/null || echo '?')
if [ "$unmatched" = 0 ]; then
  ok "mock-openai matched every request"
else
  bad "mock-openai saw $unmatched unmatched requests (see $openai/__admin/requests/unmatched)"
fi
opencode_requests=$(curl -s --max-time 30 "$openai/__admin/requests" |
  jq -r '[.requests[].request.body | fromjson? | select(.model == "mock-opencode")] | length' 2>/dev/null || echo '?')
if [ "${NO_OPENCODE:-}" = 1 ]; then
  if [ "$opencode_requests" = 0 ]; then ok "mock-openai saw no mock-opencode request"; else bad "mock-openai saw $opencode_requests mock-opencode requests, want none"; fi
else
  case $opencode_requests in
    '' | '?' | 0) bad "mock-openai saw no mock-opencode request (OpenCode did not run)" ;;
    *) ok "mock-openai saw $opencode_requests mock-opencode requests" ;;
  esac
fi

# --- git-server ------------------------------------------------------------------------------------
if [ -n "$branch" ]; then
  remote=$(git ls-remote --heads "$gitserver/$repo_path.git" "refs/heads/$branch" 2>/dev/null || true)
  if [ -n "$remote" ]; then ok "git-server has the branch $branch"; else bad "git-server does not have the branch $branch"; fi
  if [ -n "$remote" ] && [ -n "$commit" ] && [ "${remote%%[[:space:]]*}" != "$commit" ]; then
    bad "the branch is at ${remote%%[[:space:]]*}, the artifact says $commit"
  fi
  if git clone -q --depth 1 --branch "$branch" "$gitserver/$repo_path.git" "$tmp/clone" 2>"$tmp/clone.err"; then
    content=$(cat "$tmp/clone/hello.txt" 2>/dev/null || echo '<missing>')
    if [ "$content" = hello ]; then ok "hello.txt on the branch is 'hello'"; else bad "hello.txt on the branch is '$content', want 'hello'"; fi
  else
    bad "cannot clone the branch: $(head -c 300 "$tmp/clone.err")"
  fi
else
  bad "no branch to look for on git-server"
fi

finish
