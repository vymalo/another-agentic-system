#!/usr/bin/env sh
# System-level test of the default agent: one chat message becomes a pull request.
#
#   dev/coder-e2e.sh                  # OpenCode (model mock-opencode) makes the change
#   NO_OPENCODE=1 dev/coder-e2e.sh    # the check command makes it; OpenCode is not started
#   GITHUB_AUTH=app dev/coder-e2e.sh  # the stack runs the coder as a GitHub App (docker compose -f compose.yaml -f dev/compose.github-app.yaml)
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
#   * the thread exports (dev/export-thread.sh, GET /api/threads/{id}/export): a version 1 `thread-export` whose
#     job holds the pushed commit and the agent's checks passed on it, and whose log is not empty;
#   * the coder's work is a tree of steps (`steps/v1`, adam-rs e1d77be; docs/api/agui.md, "Nested steps"): the calls of
#     prepare_workspace, run_checks, commit_and_push and open_pull_request are `vymalo.step` activities, `delegate_to_opencode` is a
#     sub-agent step labelled OpenCode (a SUBAGENT_STARTED inside the coder's invocation, ended completed, finished once after its
#     last step) with at least one command or tool step running under it (OpenCode's own bash call), and the log keeps no more than
#     6 `agent_step` events of any one step (a start, at most four updates, an end). With NO_OPENCODE=1 there is no OpenCode step;
#     since adam-rs d56dd94 a tool step carries the call (steps/v1, "Input and output"): the start of prepare_workspace, run_checks,
#     commit_and_push and open_pull_request has the `input` the script sent (the repository, the check command, the commit message, the
#     title), the end is `completed` with an `output` whose text is not empty and not an error, and so does `github__list_branches`
#     (an MCP tool, the default variant: its input the repository, its output the branch `main`);
#   * the coder's answer is shown as it is written (`text-stream/v1`, adam-rs cf6ddbb; docs/api/agui.md, "Live text"): on the run
#     stream an assistant message marked `metadata["vymalo.live"]` opens, grows in at least two live deltas from offset 0, and the log's
#     final message completes the SAME message id (one START, the live deltas, the final delta, one END); what the deltas say, read by
#     offset, is the text of the one final agent_message of the log, which starts with "Opened the pull request"; a connection opened
#     after the run (the replay) reads that message once, plain, with no live frame; no artifact is named `reply`;
#   * mock-github saw exactly one POST /repos/local/sandbox/pulls, head = the branch, base = main;
#   * the coder reads GitHub through the GitHub MCP server, here the mock `mock-github-mcp` (dev/coder/coder-agent/mcp.json is
#     mounted over its folder's, and GITHUB_MCP_URL names the mock): the file holds no credential, the coder sends the credentials of
#     each call (adam-rs ADR 0017, D4). The mock's journal, which is not reset (the coder connected the server when it started, listing
#     the tools with a placeholder bearer, `ghs_adam_listing_only`), holds an `initialize` and a `tools/list`, every `tools/list` carries
#     the placeholder, and the default script (OpenCode; it reads the repository's branches with `github__list_branches` right after
#     preparing the workspace) added exactly one `tools/call` of `list_branches`, and the model was given its answer; every `tools/call`
#     of this run carries the coder's own credentials (the token below, or the installation token of an App); the variant without
#     OpenCode adds none;
#   * the coder's GitHub credential, as the stack was started with it (GITHUB_AUTH): `token` (default): every call the coder made
#     to mock-github's `/repos/...` carried `Authorization: Bearer dev-github-token`; `app`: the coder holds a GitHub App's key, no
#     token and no installation ID (GITHUB_APP_OWNERS: dev/compose.github-app.yaml); with EXPECT_INSTALLATION_LOOKUP=1 (the first run
#     after the coder started: it keeps what it found) mock-github's journal holds at least one installation lookup
#     (`GET /orgs|users/<owner>/installation`), with a JWT, for an owner on the list and no other; and its journal holds a
#     `POST /app/installations/67890/access_tokens` (the trade of a signed JWT for a token) and every call to `/repos/...`, the
#     pull request's included, carried the installation token it gave (`Bearer ghs_mockinstallationtoken...`), never the JWT;
#   * mock-openai matched every request, and saw mock-opencode requests unless NO_OPENCODE=1;
#   * git-server has the branch, and hello.txt on it is `hello`.
# The models are the scripts vendored in dev/coder/wiremock/mock-openai (see dev/coder/UPSTREAM);
# [mock:no-opencode] in the task selects the variant without OpenCode.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh;
#                    AUTH_MODE=proxy-header sends X-Auth-Request-Email to an orchestrator without the edge)
#   MOCK_GITHUB_URL  http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}
#   MOCK_GITHUB_MCP_URL  http://127.0.0.1:${MOCK_GITHUB_MCP_PORT:-8085}   (the mock's admin API; the endpoint is /mcp)
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}
#   GIT_SERVER_URL   http://127.0.0.1:${GIT_SERVER_PORT:-8093}   (from the host)
#   TIMEOUT          300    seconds to wait for the thread to end
#   NO_OPENCODE      unset  1 = the [mock:no-opencode] script
#   GITHUB_AUTH      token  how the stack was started: `token` or `app` (see the assertions above)
#   EXPECT_INSTALLATION_LOOKUP  (unset)  1 with GITHUB_AUTH=app: assert the coder found the installation (see above)
#   MOCK_GITHUB_TOKEN dev-github-token   the token of `token` mode (compose.yaml's `${MOCK_GITHUB_TOKEN-dev-github-token}`)
#
# Needs curl, jq and git (and /proc or uuidgen for a UUID). Verified by CI only, in
# .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
github=${MOCK_GITHUB_URL:-http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}}
github=${github%/}
github_mcp=${MOCK_GITHUB_MCP_URL:-http://127.0.0.1:${MOCK_GITHUB_MCP_PORT:-8085}}
github_mcp=${github_mcp%/}
github_auth=${GITHUB_AUTH:-token}
case $github_auth in
  token | app) ;;
  *) echo "GITHUB_AUTH must be token or app, not '$github_auth'" >&2; exit 2 ;;
esac
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
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
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

# mock-github-mcp's journal is kept (the coder connected the server at its start, and the stack's other runs are in it): what this
# run added is the count now minus the count here.
# mcp_count METHOD [TOOL]: the requests the mock saw with that JSON-RPC method (and tool name).
mcp_count() {
  patterns=$(jq -nc --arg m "$1" --arg t "${2:-}" '
    [{matchesJsonPath: {expression: "$.method", equalTo: $m}}]
    + (if $t == "" then [] else [{matchesJsonPath: {expression: "$.params.name", equalTo: $t}}] end)')
  curl -s --max-time 30 -X POST "$github_mcp/__admin/requests/count" -H 'Content-Type: application/json' \
    -d "{\"method\":\"POST\",\"urlPath\":\"/mcp\",\"bodyPatterns\":$patterns}" | jq -r '.count' 2>/dev/null || echo '?'
}
# model_saw_branches: the requests the scripted model got whose history holds the answer of the `github__list_branches` call
# (the tool message of call `coder-gh-1`, which names the branch main).
model_saw_branches() {
  curl -s --max-time 30 -X POST "$openai/__admin/requests/count" -H 'Content-Type: application/json' \
    -d '{"method":"POST","urlPathPattern":"(/v1)?/chat/completions","bodyPatterns":[{"matchesJsonPath":{"expression":"$.messages[?(@.tool_call_id == '"'"'coder-gh-1'"'"')].content","contains":"main"}}]}' |
    jq -r '.count' 2>/dev/null || echo '?'
}
branches_before=$(mcp_count tools/call list_branches)
calls_before=$(mcp_count tools/call)
# When this run began, in the journal's milliseconds: the bearer of a call is checked only for this run's calls (a run before it,
# as a GitHub App, carried that run's installation token, and may have ended in this very second). Where `date` has no
# milliseconds, the next whole second: this run's first call comes seconds later.
mcp_since=$(date +%s%3N 2>/dev/null)
case $mcp_since in *[!0-9]* | '') mcp_since=$((($(date +%s) + 1) * 1000)) ;; esac
saw_before=$(model_saw_branches)

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
  "$base/agui/agents/$agent_id" -H "$id_header" \
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
curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
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

# --- the export: what the owner sends a developer (GET /api/threads/{id}/export) -------------------
# dev/export-thread.sh is the script a person runs; it also checks the document's format, version and that its
# log has no gap. The job in it is the whole ledger: the pushed commit and what each source said about it.
if BASE_URL=$base AUTH_EMAIL=$email sh "$root/dev/export-thread.sh" "$thread" "$tmp/export.json" 2>"$tmp/export.err"; then
  ok "dev/export-thread.sh saved the thread ($(tail -n 1 "$tmp/export.err" | sed 's/^thread [0-9a-f-]*: //'))"
  export_facts=$(jq -r '[.format, "v\(.version)", "events>0=\(.events | length > 0)", "state=\(.thread.state)",
    "pushed=\(.job.pushed.commit // "none")",
    "checks=\([.job.results[]? | select(.source == "agent_checks") | "\(.status)@\(.commit)"] | join(","))"] | join(" ")' "$tmp/export.json" 2>/dev/null || true)
  if [ -n "$commit" ] && [ "$export_facts" = "another-agentic-system/thread-export v1 events>0=true state=done pushed=$commit checks=passed@$commit" ]; then
    ok "the export is a v1 thread-export: done, the pushed commit, the passed checks on it, a non-empty log"
  else
    bad "the export says '${export_facts:-nothing}', want a v1 thread-export, done, pushed=${commit:-<commit>}, checks=passed@${commit:-<commit>}"
  fi
else
  bad "dev/export-thread.sh failed: $(head -c 300 "$tmp/export.err")"
fi

# --- nested steps: the coder's work as a tree (steps/v1, adam-rs e1d77be) --------------------------------------------
# The orchestrator activates `steps/v1` (the coder's card lists it, read at every send), so what the coder did arrives as
# agent_step events that carry their path, not as a flood of status lines: every tool call is a step, `delegate_to_opencode` is a
# sub-agent step labelled OpenCode, and what OpenCode did under it (its bash command, its summary) are steps that run under that
# one. The projection draws the tree with AG-UI's subagents (SUBAGENT_STARTED, nested by parentSubagentRunId) and one
# `vymalo.step` activity per step (docs/api/agui.md, "Nested steps"; the golden docs/api/examples/agui/steps.agui.json), and the log
# keeps a bounded number of reports per step: the start, at most four updates and the end (docs/api/steps-v1.md). The frames are the
# thread's replay ($events), the log is the export.
# shellcheck disable=SC2016 # jq's own variables, not the shell's
steps_def='def steps: .[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step"); def under($id): (.content.path // []) | any(. == $id); '
top_tools=$(jq -r "$steps_def"'[steps | select(.content.kind == "tool" and ((.content.path // []) | length) == 0) | .content.label] | unique | join(" ")' "$events" 2>/dev/null || true)
for tool in prepare_workspace run_checks commit_and_push open_pull_request; do
  case " $top_tools " in
    *" $tool "*) ok "steps: the call of $tool is a step of its own (vymalo.step, kind tool, at the top)" ;;
    *) bad "steps: no vymalo.step for the call of $tool (top-level tool steps: ${top_tools:-none}); does the coder's card list steps/v1?" ;;
  esac
done
# What each tool step says of its call (steps/v1 "Input and output", adam-rs d56dd94; docs/api/agui.md, "Nested steps"): the step's start
# carries the call's `input` (the arguments the scripted model sent) and its end carries the `output` (`{text}`, what the tool returned).
# The replay holds one snapshot per report, the start first, the end last, and the end says the step as it stands. Only what the scripts
# fix is asserted: the input, which the script spells, and that the output is a non-empty text that is not an error; what a tool says is
# the coder's own wording (docs/api/steps-v1.md: a client draws `output.text` as text and does not parse it).
# tool_io LABEL KEY VALUE: the one top-level tool step labelled LABEL has an input whose KEY is VALUE (VALUE empty: only that KEY is
# there) on its start, and, on its end, state completed and an output whose text is not empty and is not an error.
tool_io() {
  _snaps=$(jq -c --arg l "$1" "$steps_def"'[steps | select(.content.kind == "tool" and .content.label == $l and ((.content.path // []) | length) == 0)]' "$events" 2>/dev/null || echo '[]')
  _n=$(printf '%s' "$_snaps" | jq -r 'map(.content.id) | unique | length' 2>/dev/null || echo 0)
  if [ "$_n" != 1 ]; then
    bad "steps: $_n top-level tool steps labelled $1, want exactly one"
    return 0
  fi
  _in=$(printf '%s' "$_snaps" | jq -r --arg k "$2" 'first | .content.input[$k] // empty | if type == "string" then . else tojson end' 2>/dev/null || true)
  if [ -n "$_in" ] && { [ -z "$3" ] || [ "$_in" = "$3" ]; }; then
    ok "steps: the start of $1 carries its input ($2: $_in)"
  else
    bad "steps: the start of $1 carries input.$2 '${_in:-none}', want '${3:-a value}' (input: $(printf '%s' "$_snaps" | jq -c 'first | .content.input // "none"' 2>/dev/null || true))"
  fi
  _state=$(printf '%s' "$_snaps" | jq -r 'last | .content.state // empty' 2>/dev/null || true)
  _text=$(printf '%s' "$_snaps" | jq -r 'last | .content.output.text // empty' 2>/dev/null || true)
  _err=$(printf '%s' "$_snaps" | jq -r 'last | .content.output.error // false' 2>/dev/null || true)
  if [ "$_state" = completed ] && [ -n "$_text" ] && [ "$_err" = false ]; then
    ok "steps: the end of $1 carries its output ($(printf '%s' "$_text" | wc -c | tr -d ' ') bytes of text, not an error)"
  else
    bad "steps: the end of $1 is '${_state:-none}' with output text '${_text:-none}' (error: $_err), want completed with a text that is no error"
  fi
}
if [ "${NO_OPENCODE:-}" = 1 ]; then checks_command='echo hello > hello.txt && sh ./check.sh'; else checks_command='sh ./check.sh'; fi
tool_io prepare_workspace repo_url "$repo_url"
tool_io run_checks command "$checks_command"
tool_io commit_and_push message 'feat: add hello.txt'
tool_io open_pull_request title 'feat: add hello.txt'
if [ "${NO_OPENCODE:-}" != 1 ]; then
  # An MCP tool: the mock GitHub server gives no title, so the label is the name the model knows, `<server>__<tool>`; its input is
  # the script's arguments and its output the mock's list of branches, which names `main`.
  tool_io github__list_branches repo sandbox
  branches_text=$(jq -r "$steps_def"'[steps | select(.content.kind == "tool" and .content.label == "github__list_branches")] | last | .content.output.text // empty' "$events" 2>/dev/null || true)
  case $branches_text in
    *main*) ok "steps: the output of github__list_branches names the branch main" ;;
    *) bad "steps: the output of github__list_branches does not name main ('${branches_text:-none}')" ;;
  esac
fi

opencode_ids=$(jq -r "$steps_def"'[steps | select(.content.kind == "subagent" and .content.label == "OpenCode") | .content.id] | unique | join(" ")' "$events" 2>/dev/null || true)
oid=
if [ "${NO_OPENCODE:-}" = 1 ]; then
  if [ -z "$opencode_ids" ]; then
    ok "steps: no OpenCode step ([mock:no-opencode]: OpenCode is not started)"
  else
    bad "steps: an OpenCode step ($opencode_ids) although OpenCode is not started"
  fi
else
  case $opencode_ids in
    '') bad "steps: no sub-agent step labelled OpenCode among the thread's vymalo.step activities" ;;
    *' '*) bad "steps: several OpenCode steps ($opencode_ids), the script delegates once" ;;
    *)
      oid=$opencode_ids
      ok "steps: one sub-agent step labelled OpenCode ($oid)"
      oc_state=$(jq -r --arg id "$oid" "$steps_def"'[steps | select(.content.id == $id) | .content.state] | last // empty' "$events" 2>/dev/null || true)
      if [ "$oc_state" = completed ]; then ok "steps: the OpenCode step ended completed"; else bad "steps: the OpenCode step ended '${oc_state:-none}', want completed"; fi
      # What runs under it: the activities whose path holds its id, one line per step.
      children=$(jq -r --arg id "$oid" "$steps_def"'[steps | select(under($id))] | group_by(.content.id) | map(.[0].content | "\(.kind):\(.label)") | join(" | ")' "$events" 2>/dev/null || true)
      n_children=$(jq -r --arg id "$oid" "$steps_def"'[steps | select(under($id)) | .content.id] | unique | length' "$events" 2>/dev/null || echo 0)
      if [ "${n_children:-0}" -ge 1 ]; then
        ok "steps: $n_children step(s) run under OpenCode: $children"
      else
        bad "steps: nothing runs under the OpenCode step (no vymalo.step whose path holds $oid)"
      fi
      tool_children=$(jq -r --arg id "$oid" "$steps_def"'[steps | select(under($id) and (.content.kind == "command" or .content.kind == "tool")) | .content.id] | unique | length' "$events" 2>/dev/null || echo 0)
      if [ "${tool_children:-0}" -ge 1 ]; then ok "steps: OpenCode's own tool call is one of them (a command or tool step under it)"; else bad "steps: no command or tool step under OpenCode (children: ${children:-none})"; fi
      # The tree in subagents: OpenCode is a subagent of the coder's invocation, its steps carry its own subagentRunId, and it ends once.
      sub=$(jq -r '[.[] | select(.type == "SUBAGENT_STARTED" and .name == "OpenCode")] | first | .subagentRunId // empty' "$events" 2>/dev/null || true)
      parent=$(jq -r '[.[] | select(.type == "SUBAGENT_STARTED" and .name == "OpenCode")] | first | .parentSubagentRunId // empty' "$events" 2>/dev/null || true)
      invocation=$(jq -r --arg a "$agent_id" '[.[] | select(.type == "SUBAGENT_STARTED" and .name == $a and (.parentSubagentRunId == null))] | first | .subagentRunId // empty' "$events" 2>/dev/null || true)
      if [ -n "$sub" ] && [ -n "$parent" ] && [ "$parent" = "$invocation" ]; then
        ok "steps: OpenCode is a subagent ($sub) of the $agent_id invocation ($invocation)"
      else
        bad "steps: SUBAGENT_STARTED OpenCode is '${sub:-none}' in '${parent:-none}', want it inside the $agent_id invocation '${invocation:-none}'"
      fi
      own=$(jq -r --arg id "$oid" --arg sub "$sub" "$steps_def"'[steps | select(under($id)) | .subagentRunId] | unique | if . == [$sub] then "yes" else "no: \(tostring)" end' "$events" 2>/dev/null || true)
      if [ -n "$sub" ] && [ "$own" = yes ]; then ok "steps: the steps under OpenCode are attributed to its subagent"; else bad "steps: the steps under OpenCode carry the subagentRunId $own, want $sub"; fi
      # It ends once, after its last step, and not as an error.
      closing=$(jq -r --arg sub "$sub" 'to_entries as $e
        | ([$e[] | select(.value.type == "SUBAGENT_FINISHED" and .value.subagentRunId == $sub)] | map(.key)) as $fin
        | ([$e[] | select(.value.type == "SUBAGENT_ERROR" and .value.subagentRunId == $sub)] | length) as $err
        | ([$e[] | select(.value.type == "ACTIVITY_SNAPSHOT" and .value.activityType == "vymalo.step" and .value.subagentRunId == $sub)] | map(.key) | max // -1) as $lastchild
        | "\($fin | length) \($err) \($lastchild) \($fin | first // -1)"' "$events" 2>/dev/null || true)
      # closing = <SUBAGENT_FINISHED count> <SUBAGENT_ERROR count> <frame of its last step> <frame of its SUBAGENT_FINISHED>
      # shellcheck disable=SC2086 # four numbers, split on purpose
      set -- $closing
      if [ "$#" = 4 ] && [ "$1" = 1 ] && [ "$2" = 0 ] && [ "$3" -ge 0 ] && [ "$4" -gt "$3" ]; then
        ok "steps: the OpenCode subagent finished once, after its last step"
      else
        bad "steps: the OpenCode subagent: '${closing:-nothing}' (finished, errors, last step frame, finished at frame), want 1 0 and a finish after the last step"
      fi
      ;;
  esac
fi
# The log: the same tree, bounded (a retry would start a step again, and the script has none).
if [ -s "$tmp/export.json" ]; then
  logged=$(jq -r '[.events[] | select(.kind == "agent_step")] | length' "$tmp/export.json" 2>/dev/null || echo 0)
  most=$(jq -r '[.events[] | select(.kind == "agent_step") | .data.id] | group_by(.) | map(length) | max // 0' "$tmp/export.json" 2>/dev/null || echo 99)
  if [ "${logged:-0}" -ge 1 ] && [ "$most" -le 6 ]; then
    ok "steps: the log holds $logged agent_step events and no step has more than $most of them (a start, at most four updates, an end: at most 6)"
  else
    bad "steps: the log holds ${logged:-0} agent_step events and one step has $most, want some, and at most 6 of any one step"
  fi
  if [ -n "$oid" ]; then
    shape=$(jq -r --arg id "$oid" '[.events[] | select(.kind == "agent_step" and .data.id == $id) | .data.phase] | "\(first // "none")..\(last // "none")"' "$tmp/export.json" 2>/dev/null || true)
    in_log=$(jq -r --arg id "$oid" '[.events[] | select(.kind == "agent_step" and (.data.path | any(. == $id))) | .data.id] | unique | length' "$tmp/export.json" 2>/dev/null || echo 0)
    if [ "$shape" = start..end ] && [ "${in_log:-0}" -ge 1 ]; then
      ok "steps: the log has the OpenCode step from its start to its end, and $in_log step(s) whose path holds it"
    else
      bad "steps: in the log the OpenCode step runs '$shape' with $in_log step(s) under it, want start..end and at least one"
    fi
  fi
else
  bad "steps: no export to read the log's agent_step events from"
fi

# --- the answer, as it was written (text-stream/v1, adam-rs cf6ddbb) ----------------------------------------------------
# The coder streams its model calls and the orchestrator activates `text-stream/v1` (the card lists it), so the last answer
# reaches the run stream while it is written: TEXT_MESSAGE_* frames marked metadata["vymalo.live"] (docs/api/agui.md, "Live
# text"; the golden docs/api/examples/agui/stream.agui.json), and then the log's message, final, completes the SAME message id.
# The pieces are never in the log, so they are in the run stream, not in the replay ($events): the replay holds the message once,
# plain. The script's last answer is a text the mock model dribbles over about two seconds, so it comes in several pieces.
# `reading` is how a client keeps a message: a live delta continues from its offset (UTF-16 code units; the mock's words are ASCII,
# so they are jq's string positions), any other delta is appended.
# shellcheck disable=SC2016 # jq's own variables, not the shell's
reading='reduce (.[] | select(.type == "TEXT_MESSAGE_CONTENT" and .messageId == $id)) as $c ("";
  (if $c.metadata["vymalo.live"].offset != null then .[0:$c.metadata["vymalo.live"].offset] else . end) + $c.delta)'
run_frames=$tmp/run-frames.json
sse_events "$tmp/run.sse" | jq -s '.' > "$run_frames" 2>/dev/null || echo '[]' > "$run_frames"
live_id=$(jq -r '[.[] | select(.type == "TEXT_MESSAGE_START" and .role == "assistant" and .metadata["vymalo.live"] != null) | .messageId] | last // empty' "$run_frames" 2>/dev/null || true)
if [ -z "$live_id" ]; then
  bad "live text: no assistant message marked vymalo.live on the run stream: the coder's answer was not shown as it was written (does its card list text-stream/v1? frames: $(jq -c '[.[].type] | group_by(.) | map("\(.[0]) \(length)")' "$run_frames" 2>/dev/null | head -c 400))"
else
  ok "live text: the run stream opened a live message ($live_id)"
  shape=$(jq -r --arg id "$live_id" '[.[] | select(.messageId == $id and (.type | startswith("TEXT_MESSAGE_"))) | if .type == "TEXT_MESSAGE_START" then "S" elif .type == "TEXT_MESSAGE_END" then "E" elif (.metadata["vymalo.live"].final // false) then "F" else "c" end] | join("")' "$run_frames" 2>/dev/null || true)
  pieces=$(printf '%s' "$shape" | tr -cd c | wc -c | tr -d ' ')
  if [ "$pieces" -ge 2 ]; then ok "live text: the answer grew in $pieces live deltas"; else bad "live text: the answer came in $pieces live delta(s) (frames: $shape), want at least 2"; fi
  if printf '%s' "$shape" | grep -Eq '^Sc+FE$'; then
    ok "live text: one START, the live deltas, then the log's final delta and one END, all under the same message id"
  else
    bad "live text: the frames of the message are '$shape', want one START (S), the live deltas (c), the final delta (F) and one END (E): ScccFE"
  fi
  first_offset=$(jq -r --arg id "$live_id" '[.[] | select(.type == "TEXT_MESSAGE_CONTENT" and .messageId == $id and .metadata["vymalo.live"] != null)] | first | .metadata["vymalo.live"].offset // "none"' "$run_frames" 2>/dev/null || true)
  if [ "$first_offset" = 0 ]; then ok "live text: the first piece begins at offset 0"; else bad "live text: the first piece begins at offset '$first_offset', want 0"; fi
  read_live=$(jq -r --arg id "$live_id" "$reading" "$run_frames" 2>/dev/null || true)
  final_text=
  if [ -s "$tmp/export.json" ]; then
    final_text=$(jq -r --arg id "$live_id" '[.events[] | select(.kind == "agent_message" and .data.messageId == $id and .data.final == true) | .data.text] | first // empty' "$tmp/export.json" 2>/dev/null || true)
    finals=$(jq -r --arg id "$live_id" '[.events[] | select(.kind == "agent_message" and .data.messageId == $id and .data.final == true)] | length' "$tmp/export.json" 2>/dev/null || echo '?')
    partials=$(jq -r '[.events[] | select(.kind == "agent_message" and .data.final == false)] | length' "$tmp/export.json" 2>/dev/null || echo '?')
    if [ "$finals" = 1 ] && [ "$partials" = 0 ]; then
      ok "live text: the log holds the answer once, as one final agent_message under the live message's id, and no partial one"
    else
      bad "live text: the log holds $finals final agent_message(s) under $live_id and $partials partial one(s), want 1 and 0"
    fi
  fi
  case $final_text in
    "Opened the pull request"*) ok "live text: the log's text is the coder's answer: $final_text" ;;
    *) bad "live text: the log's text under $live_id is '$final_text', want the coder's answer (Opened the pull request ...)" ;;
  esac
  if [ -n "$final_text" ] && [ "$read_live" = "$final_text" ]; then
    ok "live text: the deltas read by offset, then completed by the log's message, are the logged text"
  else
    bad "live text: the run stream reads '$read_live', the log says '$final_text'"
  fi
  replay_live=$(jq -r '[.[] | select(.metadata["vymalo.live"] != null)] | length' "$events" 2>/dev/null || echo '?')
  replay_starts=$(jq -r --arg id "$live_id" '[.[] | select(.type == "TEXT_MESSAGE_START" and .messageId == $id)] | length' "$events" 2>/dev/null || echo '?')
  read_replay=$(jq -r --arg id "$live_id" "$reading" "$events" 2>/dev/null || true)
  if [ "$replay_live" = 0 ] && [ "$replay_starts" = 1 ] && [ "$read_replay" = "$final_text" ]; then
    ok "live text: a connection opened after the run reads the answer once, plain, with no live frame"
  else
    bad "live text: the replay holds $replay_live live frame(s), starts the answer $replay_starts time(s) and reads '$read_replay', want 0, 1 and '$final_text'"
  fi
fi
# The chunks the agent sends are not a tool's artifact: they are the live words, and neither the run nor the replay shows one.
reply_artifacts=$(jq -s '[.[][] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact" and .content.name == "reply")] | length' "$run_frames" "$events" 2>/dev/null || echo '?')
if [ "$reply_artifacts" = 0 ]; then ok "live text: no artifact named reply (the chunks are live words, not an artifact)"; else bad "live text: $reply_artifacts vymalo.artifact(s) named reply, want none"; fi

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

# --- the GitHub MCP server (mock-github-mcp) ----------------------------------------------------------------
# The coder connected it when it started: `initialize`, then `tools/list`.
inits=$(mcp_count initialize)
lists=$(mcp_count tools/list)
if [ "$inits" != '?' ] && [ "$inits" -ge 1 ]; then ok "mock-github-mcp saw initialize ($inits)"; else bad "mock-github-mcp saw $inits initialize, want at least 1 (is the stack started with the dev mcp.json mounted over the coder's folder?)"; fi
if [ "$lists" != '?' ] && [ "$lists" -ge 1 ]; then ok "mock-github-mcp saw tools/list ($lists)"; else bad "mock-github-mcp saw $lists tools/list, want at least 1"; fi
branches_after=$(mcp_count tools/call list_branches)
calls_after=$(mcp_count tools/call)
if [ "${NO_OPENCODE:-}" != 1 ]; then
  # The default script reads the branches of the repository right after preparing the workspace.
  if [ "$branches_before" != '?' ] && [ "$branches_after" != '?' ] && [ $((branches_after - branches_before)) -eq 1 ]; then
    ok "mock-github-mcp saw exactly one tools/call of list_branches in this run"
  else
    bad "mock-github-mcp saw $branches_before then $branches_after tools/call of list_branches, want exactly one more"
  fi
  # The dev mcp.json holds no credential: the coder gives each call the credentials of that call (adam-rs ADR 0017, D4), the token
  # of the stack or, as a GitHub App, the installation token it minted. The listing at startup carries a placeholder and no call does.
  case "$github_auth" in
    token) want_bearer="Bearer ${MOCK_GITHUB_TOKEN-dev-github-token}"; how=exact ;;
    app) want_bearer='Bearer ghs_mockinstallationtoken'; how=prefix ;;
  esac
  # mcp_wrong_bearers <JSON-RPC method> <wanted bearer> <exact|prefix> [since ms]: how many requests of the journal with that
  # method (logged at or after `since`, when given) carry another `Authorization` (or none), or ? when the mock does not answer.
  mcp_wrong_bearers() {
    patterns=$(jq -nc --arg m "$1" '[{matchesJsonPath: {expression: "$.method", equalTo: $m}}]')
    curl -s --max-time 30 -X POST "$github_mcp/__admin/requests/find" -H 'Content-Type: application/json' \
      -d "{\"method\":\"POST\",\"urlPath\":\"/mcp\",\"bodyPatterns\":$patterns}" |
      jq -r --arg want "$2" --arg how "$3" --argjson since "${4:-0}" '[.requests[] | select((.loggedDate // 0) >= $since) | .headers | with_entries(.key |= ascii_downcase) | (.authorization // "") | select(if $how == "exact" then . != $want else (startswith($want) | not) end)] | length' 2>/dev/null || echo '?'
  }
  wrong=$(mcp_wrong_bearers tools/call "$want_bearer" "$how" "$mcp_since")
  if [ "$wrong" = 0 ]; then ok "every tools/call to mock-github-mcp carried the coder's credentials ('$want_bearer...')"; else bad "$wrong tools/call request(s) to mock-github-mcp did not carry '$want_bearer'"; fi
  wrong=$(mcp_wrong_bearers tools/list 'Bearer ghs_adam_listing_only' exact)
  if [ "$wrong" = 0 ]; then ok "every tools/list to mock-github-mcp carried the startup placeholder, not a credential"; else bad "$wrong tools/list request(s) to mock-github-mcp did not carry the placeholder 'Bearer ghs_adam_listing_only'"; fi
  # And the model was given what it answered: the branch main.
  saw_after=$(model_saw_branches)
  if [ "$saw_before" != '?' ] && [ "$saw_after" != '?' ] && [ "$saw_after" -gt "$saw_before" ]; then
    ok "the model was given the answer of github__list_branches (the branch main)"
  else
    bad "the model was not given the answer of github__list_branches ($saw_before then $saw_after requests with it)"
  fi
else
  if [ "$calls_before" != '?' ] && [ "$calls_after" = "$calls_before" ]; then ok "the variant without OpenCode reads nothing over MCP: mock-github-mcp saw no tools/call"; else bad "mock-github-mcp saw tools/call go from $calls_before to $calls_after, want no change"; fi
fi

# --- the coder's GitHub credential -----------------------------------------------------------------------------
# Every call the coder made to the repositories' API in this run (mock-github's journal was reset at the start): which `Authorization` it carried.
repo_calls=$tmp/repo-calls.json
curl -s --max-time 30 -X POST "$github/__admin/requests/find" -H 'Content-Type: application/json' \
  -d '{"urlPathPattern":"/repos/.*"}' > "$repo_calls" || true
n_calls=$(jq -r '.requests | length' "$repo_calls" 2>/dev/null || echo 0)
auths=$(jq -r '.requests[] | .headers | with_entries(.key |= ascii_downcase) | .authorization // "none"' "$repo_calls" 2>/dev/null | sort | uniq -c | sed 's/^ *//' | tr '\n' ';')
case $github_auth in
  token)
    want="Bearer ${MOCK_GITHUB_TOKEN-dev-github-token}"
    wrong=$(jq -r --arg want "$want" '[.requests[] | .headers | with_entries(.key |= ascii_downcase) | select(.authorization != $want)] | length' "$repo_calls" 2>/dev/null || echo '?')
    if [ "$n_calls" -ge 1 ] && [ "$wrong" = 0 ]; then ok "all $n_calls call(s) to the repositories' API carried the token"; else bad "token mode: $wrong of $n_calls call(s) to /repos/... did not carry '$want' ($auths)"; fi
    ;;
  app)
    # The coder holds no pin (GITHUB_APP_OWNERS): it found the installation of the repository's owner with the App's JWT. It keeps what it
    # found, so only the first run after the coder started sees the lookup (the CI job says so: EXPECT_INSTALLATION_LOOKUP=1); the trade
    # of a token is seen by every run, since the mock's tokens last four minutes.
    if [ "${EXPECT_INSTALLATION_LOOKUP:-}" = 1 ]; then
      lookups=$tmp/lookups.json
      curl -s --max-time 30 -X POST "$github/__admin/requests/find" \
        -H 'Content-Type: application/json' \
        -d '{"method":"GET","urlPathPattern":"/(orgs|users)/[^/]+/installation"}' > "$lookups" || true
      n_lookups=$(jq -r '.requests | length' "$lookups" 2>/dev/null || echo 0)
      not_jwt=$(jq -r '[.requests[] | .headers | with_entries(.key |= ascii_downcase) | select((.authorization // "") | test("^Bearer eyJ[A-Za-z0-9_-]+\\.[A-Za-z0-9_-]+\\.[A-Za-z0-9_-]+$") | not)] | length' "$lookups" 2>/dev/null || echo '?')
      if [ "$n_lookups" -ge 1 ] && [ "$not_jwt" = 0 ]; then ok "the coder found the installation of the owner with the App's JWT ($n_lookups lookup(s) at /orgs|users/<owner>/installation)"; else bad "installation lookups: $n_lookups, of which $not_jwt without a JWT; want at least 1, all with a JWT (is the stack started with -f dev/compose.github-app.yaml, GITHUB_APP_OWNERS and no installation ID, and is this the first run after the coder started?)"; fi
      listed=$(jq -r '[.requests[] | .url | split("/")[2]] | unique | join(" ")' "$lookups" 2>/dev/null || true)
      for owner in $listed; do
        case "$owner" in local | scratch | other-org) ;; *) bad "the coder looked up the installation of '$owner', which is not on GITHUB_APP_OWNERS" ;; esac
      done
    fi
    mints=$(curl -s --max-time 30 -X POST "$github/__admin/requests/find" -H 'Content-Type: application/json' \
      -d '{"method":"POST","urlPath":"/app/installations/67890/access_tokens"}' | jq -r '.requests | length' 2>/dev/null || echo '?')
    if [ "$mints" != '?' ] && [ "$mints" -ge 1 ]; then ok "the coder traded a JWT for an installation token ($mints POST /app/installations/67890/access_tokens)"; else bad "mock-github saw $mints POST /app/installations/67890/access_tokens, want at least 1 (is the stack started with -f dev/compose.github-app.yaml?)"; fi
    wrong=$(jq -r '[.requests[] | .headers | with_entries(.key |= ascii_downcase) | select((.authorization // "") | startswith("Bearer ghs_mockinstallationtoken") | not)] | length' "$repo_calls" 2>/dev/null || echo '?')
    if [ "$n_calls" -ge 1 ] && [ "$wrong" = 0 ]; then ok "all $n_calls call(s) to the repositories' API carried the installation token"; else bad "app mode: $wrong of $n_calls call(s) to /repos/... did not carry 'Bearer ghs_mockinstallationtoken...' ($auths)"; fi
    ;;
esac

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
