#!/usr/bin/env sh
# System-level test of the coder's workspaces (MVP slice 7): a task that names no repository, a repository the coder
# creates when the person says yes, and a second repository that joins a workspace only when the person says yes. The
# person answers each question through the web's own path: the coder draws it as ONE form from the screen's UI catalog
# (a Choices with a `consent` question, options `yes` and `no`), and one A2UI action answers it, exactly as
# dev/choices-e2e.sh does for its three questions. Nothing here talks to the coder: the whole chain goes through the
# orchestrator and the edge, and the git, GitHub and CI sides are the stack's mocks.
#
#   dev/workspace-e2e.sh                          # the four threads below
#   SCENARIOS="second-repo" dev/workspace-e2e.sh  # only these (create-repo create-repo-no second-repo second-repo-no)
#   GITHUB_AUTH=app dev/workspace-e2e.sh          # the stack runs the coder as a GitHub App (dev/compose.github-app.yaml)
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# The script speaks AG-UI, as the web does (docs/api/agui.md), with one new thread per scenario on the default agent `coder`:
# POST /agui/agents/coder, the thread's state from GET /api/threads/{id}, its frames from GET /agui/threads/{id}/connect?mode=run,
# its log from GET /api/threads/{id}/export. The coder's model is `mock-coder`: a task that carries `[mock:create-repo]` or
# `[mock:second-repo]` selects the script (dev/coder/wiremock/mock-openai/mappings/coder-script.json, vendored from adam-rs).
# The catalog it sends is the one the web ships (web/src/features/chat/lib/a2ui/catalog/catalog.json and catalog.lock.json), as
# `forwardedProps["vymalo.uiCatalog"]` of the first run; the thread holds it afterwards.
#
# create-repo (the person says yes): "Write a fib.sh ... I'll give you a repo later." The coder builds the project in a scratch
# project and, with no repository named, stops with a question: the thread is `blocked`, and nothing was pushed, opened or created
# (no pull request, no POST /orgs/scratch/repos on mock-github, git-server has never heard of the repository). Then the person asks
# for `scratch/fib-<id>` in words, a plain message; the coder calls `create_repository`, whose question the TOOL writes: the thread is
# `blocked` again, on one a2ui-surface with a Choices of one question `consent`, options `yes` and `no`, and still nothing was created.
# One action answers `consent = yes`, and then:
#   * the thread ends `done` and the run ends RUN_FINISHED (success);
#   * exactly one POST /orgs/scratch/repos reached mock-github, AFTER the answer (it was zero before), for that name, private, empty;
#   * the thread recorded the answer (a vymalo.action with `consent = yes`);
#   * the coder's work reached the repository it created: `main` is the one empty commit, the agent's branch is on git-server at the
#     commit the `branch` artifact names, and fib.sh on it prints the first seven Fibonacci numbers;
#   * the last `checks` artifact passed on exactly the pushed commit, and the job runs under the gate `ci+agent_checks`: an
#     `agent_checks` vymalo.check card passed on the pushed commit, and one vymalo.ci card, `mock-ci/build`, `success`, for that
#     commit and for `scratch/fib-<id>` (mock-ci found the new repository by itself: MOCK_CI_REPOS `scratch/*`);
#   * mock-github saw exactly one POST /repos/scratch/fib-<id>/pulls, head = the branch, base = main;
#   * the coder's credential, as the stack was started with it (GITHUB_AUTH, as dev/coder-e2e.sh): every call to mock-github's
#     /repos/... and /orgs/... carried it (`token`: Bearer dev-github-token; `app`: the installation token a signed JWT was traded for,
#     and a POST /app/installations/67890/access_tokens is in the journal); and the credential is nowhere in the thread's export.
# create-repo-no is the same up to the question, and answers `no`: the thread stays `blocked` (the coder says it did not create the
# repository), mock-github never saw a POST /orgs/scratch/repos, git-server never heard of the repository and no pull request exists.
#
# second-repo (yes): the task names local/sandbox and asks for the shared greeting, which lives in local/library (seeded, never
# named): the coder prepares the sandbox and calls `request_repository`, so the thread is `blocked` on a Choices (`consent`, options
# `Yes, add local/library` and `No`) whose question names the repository and quotes the reason, no pull request exists, and git-server
# has not been asked for local/library since the script began (the repository is seeded, so only git-server's access log tells:
# GIT_SERVER_LOGS). After `yes`: the thread ends `done`, hello.txt on the branch holds the library's greeting, git-server was asked
# for local/library AFTER the answer, and the gate, the checks and the one pull request hold as above, for local/sandbox.
# second-repo-no: after `no` git-server was NEVER asked for local/library, no pull request was opened, and the thread is `blocked`
# again (the coder says it could not add the library).
#
# If a consent question comes as text (no a2ui-surface: the catalog did not reach the coder, or it fell back), the script says so in a
# NOTE, counts it as a FAIL, and answers with the text `yes` or `no` so that the rest of the chain is still checked.
#
# It prints one ok or FAIL line per check and exits 1 if any failed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_GITHUB_URL  http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}   (its journal is emptied at the start of each scenario)
#   GIT_SERVER_URL   http://127.0.0.1:${GIT_SERVER_PORT:-8093}    (from the host)
#   CATALOG_FILE     web/src/features/chat/lib/a2ui/catalog/catalog.json        the screen's catalog
#   CATALOG_LOCK     web/src/features/chat/lib/a2ui/catalog/catalog.lock.json   its {version, digest}
#   TIMEOUT          300    seconds to wait for a run to end
#   SCENARIOS        create-repo create-repo-no second-repo second-repo-no
#   GITHUB_AUTH      token  how the stack was started: `token` or `app`
#   MOCK_GITHUB_TOKEN dev-github-token   the token of `token` mode (compose.yaml's `${MOCK_GITHUB_TOKEN-dev-github-token}`)
#   GIT_SERVER_LOGS  docker compose -f compose.yaml --profile app logs --no-color git-server   a command that prints git-server's
#                    access log (second-repo: which repositories it was asked for); set it when docker is not here, or the
#                    check is skipped
#   REPO_BASE_URL    http://git-server:8080   where the coder (inside the compose network) finds git-server
#
# Needs curl, jq and git (and /proc or uuidgen for a UUID). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$(dirname "$0")/auth-header.sh" "$email")
github=${MOCK_GITHUB_URL:-http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}}
github=${github%/}
gitserver=${GIT_SERVER_URL:-http://127.0.0.1:${GIT_SERVER_PORT:-8093}}
gitserver=${gitserver%/}
repo_base=${REPO_BASE_URL:-http://git-server:8080}
repo_base=${repo_base%/}
timeout=${TIMEOUT:-300}
scenarios=${SCENARIOS:-create-repo create-repo-no second-repo second-repo-no}
github_auth=${GITHUB_AUTH:-token}
case $github_auth in
  token | app) ;;
  *) echo "GITHUB_AUTH must be token or app, not '$github_auth'" >&2; exit 2 ;;
esac
for s in $scenarios; do
  case $s in
    create-repo | create-repo-no | second-repo | second-repo-no) ;;
    *) echo "unknown scenario '$s'; choose from: create-repo create-repo-no second-repo second-repo-no" >&2; exit 2 ;;
  esac
done

root=$(cd "$(dirname "$0")/.." && pwd)
catalog_file=${CATALOG_FILE:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.json}
catalog_lock=${CATALOG_LOCK:-$root/web/src/features/chat/lib/a2ui/catalog/catalog.lock.json}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
note() { echo "NOTE $1"; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "workspace e2e passed"; else echo "workspace e2e FAILED"; exit 1; fi
}

for f in "$catalog_file" "$catalog_lock"; do
  if [ ! -f "$f" ]; then
    echo "FAIL $f does not exist (CATALOG_FILE and CATALOG_LOCK name the screen's catalog and its lock)"
    exit 1
  fi
done
catalog_version=$(jq -r '.version' "$catalog_lock")
catalog_digest=$(jq -r '.digest' "$catalog_lock")

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, one JSON per line
  sed -n 's/^data: *//p' "$1"
}

# --- the agent -----------------------------------------------------------------------------------------------
if agents=$(api GET /api/agents 2>"$tmp/err"); then
  if printf '%s' "$agents" | jq -e 'any(.[]; .id == "coder")' >/dev/null 2>&1; then
    ok "GET /api/agents lists coder"
  else
    bad "GET /api/agents does not list coder (agents: $(printf '%s' "$agents" | jq -c '[.[].id]'))"
    finish
  fi
else
  bad "GET /api/agents: $(head -c 300 "$tmp/err") $agents"
  finish
fi

# --- one run -------------------------------------------------------------------------------------------------
thread=
# stream INPUT_FILE LABEL: POST the RunAgentInput and wait for the run to end, then for the thread to settle. Sets
#   outcome   how the run stream ended (success, interrupt, error: <code>, or empty)
#   state     the state the thread ended in
#   said      the words the coder spoke in this run, the assistant messages joined with a space (a live message as a client keeps it)
#   events    the file holding every frame of the thread so far (a JSON array)
stream() {
  _deadline=$(( $(date +%s) + timeout ))
  _code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/coder" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary "@$1" 2>"$tmp/err" || true)
  if [ "$_code" != 200 ]; then
    bad "$2: POST /agui/agents/coder answered HTTP ${_code:-none}: $(head -c 300 "$tmp/err") $(head -c 400 "$tmp/run.sse" 2>/dev/null)"
    outcome=
    state=
    said=
    echo '[]' > "$tmp/events.json"
    events=$tmp/events.json
    return 0
  fi
  outcome=$(sse_events "$tmp/run.sse" | jq -rs '[.[] | select(.type == "RUN_FINISHED" or .type == "RUN_ERROR")] | last
    | if . == null then "" elif .type == "RUN_ERROR" then "error: \(.code // "")" else (.outcome.type // "success") end' 2>/dev/null || true)
  # A live delta continues from its offset (UTF-16 code units; the mock's words are ASCII, so jq's string positions), any other delta is appended.
  said=$(sse_events "$tmp/run.sse" | jq -rs 'reduce .[] as $f ({order: [], text: {}};
      if $f.type == "TEXT_MESSAGE_START" and $f.role == "assistant" then
        (if (.text | has($f.messageId)) then . else (.order += [$f.messageId] | .text[$f.messageId] = "") end)
      elif $f.type == "TEXT_MESSAGE_CONTENT" and (.text | has($f.messageId)) then
        .text[$f.messageId] |= ((if $f.metadata["vymalo.live"].offset != null then .[0:$f.metadata["vymalo.live"].offset] else . end) + $f.delta)
      else . end)
    | [.order[] as $id | .text[$id]] | join(" ")' 2>/dev/null || true)
  state=
  while :; do
    state=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '.state // empty' || true)
    case $state in done | blocked | failed | cancelled) break ;; esac
    if [ "$(date +%s)" -ge "$_deadline" ]; then break; fi
    sleep 2
  done
  events=$tmp/events.json
  curl -sS --max-time 60 -H "$id_header" -H 'accept: text/event-stream' \
    "$base/agui/threads/$thread/connect?mode=run" 2>/dev/null | sed -n 's/^data: *//p' | jq -s '.' > "$events" 2>/dev/null ||
    echo '[]' > "$events"
  echo "$2: the coder said: ${said:-<nothing>}"
}

# why EVENTS: what the thread said about a failure, to help whoever reads the log.
why() {
  jq -r '.[] | select(.type == "RUN_ERROR" or (.type == "ACTIVITY_SNAPSHOT" and (.activityType == "vymalo.status" or .activityType == "vymalo.error")))
         | "     \(.type) \(.activityType // "") \(.content.status // "") \(.content.message // .content.detail // .message // "")"' "$1" | head -n 20
}

# first_message TEXT FILE: the RunAgentInput of the first run: one user message and the screen's catalog (forwardedProps["vymalo.uiCatalog"]).
first_message() {
  jq -c --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" \
    --argjson version "$catalog_version" --arg digest "$catalog_digest" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [],
      messages: [{id: $msg, role: "user", content: $text}],
      forwardedProps: {"vymalo.uiCatalog": {catalogId: .catalogId, version: $version, digest: $digest, catalog: .}}}' "$catalog_file" > "$2"
}

# next_message TEXT FILE: a later run with one new user message (the thread holds the catalog: the orchestrator tells the agent about it on each message).
next_message() {
  jq -nc --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}' > "$2"
}

# action_message FILE: the RunAgentInput that answers the form of $surface_id: ONE action, as the web sends it, whose context is
# {answers: [{id, values}]} in question order ($answers_json). Same shape as dev/choices-e2e.sh's run 2.
action_message() {
  jq -nc --arg thread "$thread" --arg run "$(uuid)" --arg surface "$surface_id" --arg source "$choices_id" --arg name "$event_name" \
    --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson answers "$answers_json" '{
      threadId: $thread, runId: $run, state: {}, tools: [], context: [], messages: [],
      forwardedProps: {a2uiAction: {userAction: {
        name: $name, surfaceId: $surface, sourceComponentId: $source, timestamp: $ts, context: {answers: $answers}}}}}' > "$1"
}

# read_surfaces: from $events, sets
#   surfaces     how many a2ui-surface activities the thread has
#   choices      the Choices components of the last snapshot of the last surface, a JSON array
#   surface_id, choices_id, event_name   what an answer to it must name
read_surfaces() {
  surfaces=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface") | .messageId] | unique | length' "$events" 2>/dev/null || echo '?')
  _ops=$(jq -c '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "a2ui-surface")] | last | .content.a2ui_operations // []' "$events" 2>/dev/null || echo '[]')
  surface_id=$(printf '%s' "$_ops" | jq -r '[.[] | .createSurface? // empty] | first | .surfaceId // empty')
  choices=$(printf '%s' "$_ops" | jq -c '[.[] | .updateComponents? // empty | .components[]? | select(.component == "Choices")]')
  choices_id=$(printf '%s' "$choices" | jq -r '.[0].id // empty')
  event_name=$(printf '%s' "$choices" | jq -r '.[0].action.event.name // "answer"')
}

# expect_consent LABEL YES_LABEL: the thread waits on ONE surface that holds one Choices with the question `consent`, options yes and
# no, the yes labelled YES_LABEL (a substring). Sets surface_id, choices_id, event_name (empty when there is no form).
expect_consent() {
  read_surfaces
  if [ "$surfaces" = 1 ] && [ -n "$surface_id" ] && [ "$(printf '%s' "$choices" | jq -r 'length')" = 1 ]; then
    ok "$1: the thread waits on one a2ui-surface with one Choices"
    if printf '%s' "$choices" | jq -e '.[0].questions | length == 1 and .[0].id == "consent" and ([.[0].options[].value] == ["yes", "no"])' >/dev/null 2>&1; then
      ok "$1: the form asks one question, consent, and offers yes and no"
    else
      bad "$1: the Choices is not one question 'consent' with the options yes and no: $(printf '%s' "$choices" | head -c 500)"
    fi
    if printf '%s' "$choices" | jq -e --arg l "$2" '.[0].questions[0].options[0].label | contains($l)' >/dev/null 2>&1; then
      ok "$1: the yes option says '$2'"
    else
      bad "$1: the yes option is '$(printf '%s' "$choices" | jq -r '.[0].questions[0].options[0].label // "none"')', want it to say '$2'"
    fi
  else
    bad "$1: the consent question did not come as a form: $surfaces a2ui-surface activities, $(printf '%s' "$choices" | jq -r 'length') Choices (does the coder's message carry the screen's catalog?)"
    note "$1: answering with text instead of the form's action"
    surface_id=
    choices_id=
  fi
}

# answer_consent LABEL yes|no: the person's answer, as the web sends it (one action), or as words when there was no form.
answer_consent() {
  if [ -n "$surface_id" ] && [ -n "$choices_id" ]; then
    answers_json=$(jq -nc --arg v "$2" '[{id: "consent", values: [$v]}]')
    action_message "$tmp/answer.json"
  else
    next_message "$2" "$tmp/answer.json"
  fi
  stream "$tmp/answer.json" "$1"
}

# recorded_answer: the last vymalo.action of the thread, as `consent=<value>` ("" when none).
recorded_answer() {
  jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.action")] | last
    | .content.context.answers // [] | map("\(.id)=\(.values | join(","))") | join(" ")' "$events" 2>/dev/null || true
}

# --- the mocks -----------------------------------------------------------------------------------------------
reset_github() {
  _c=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$github/__admin/requests" || true)
  if [ "$_c" = 200 ]; then ok "$1: mock-github's journal reset"; else bad "$1: mock-github's journal reset answered HTTP $_c"; fi
}

# github_find METHOD URL_PATH_PATTERN FILE: the requests of mock-github's journal that match, in FILE.
github_find() {
  curl -s --max-time 30 -X POST "$github/__admin/requests/find" -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg m "$1" --arg p "$2" '{method: $m, urlPathPattern: $p}')" > "$3" || true
}

# count_of METHOD URL_PATH_PATTERN: how many such requests mock-github saw since its journal was reset (`?` when it cannot be read).
count_of() {
  github_find "$1" "$2" "$tmp/found.json"
  jq -r '.requests | length' "$tmp/found.json" 2>/dev/null || echo '?'
}

# listed NAME.git: how often git-server's listing of the scratch owner (JSON) has it; 0 when the owner has no repository yet (a 404).
listed() {
  _lc=$(curl -s -o "$tmp/listing.json" -w '%{http_code}' --max-time 30 "$gitserver/__repos/scratch/" || true)
  case $_lc in
    404) echo 0 ;;
    200) jq -r --arg n "$1" '[.[]? | select(.name == $n)] | length' "$tmp/listing.json" 2>/dev/null || echo '?' ;;
    *) echo '?' ;;
  esac
}

# library_requests: how many lines of git-server's access log name the library repository, or `?` when the log cannot be read. The
# repository is seeded, so only the log tells whether it was asked for. mock-ci does not watch it (MOCK_CI_REPOS in compose.yaml).
library_requests() {
  if [ -n "${GIT_SERVER_LOGS:-}" ]; then
    _logs=$(sh -c "$GIT_SERVER_LOGS" 2>/dev/null) || { echo '?'; return; }
  elif command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
    _logs=$(docker compose -f "$root/compose.yaml" --profile app logs --no-color git-server 2>/dev/null) || { echo '?'; return; }
  else
    echo '?'; return
  fi
  printf '%s\n' "$_logs" | grep -c '/local/library.git' || true
}

# dump_logs: the tail of the logs that say why a CI report did not reach its job, when docker is here.
dump_logs() {
  command -v docker >/dev/null 2>&1 || return 0
  for service in orchestrator mock-ci git-server; do
    echo "     --- docker compose logs --tail 40 $service"
    docker compose -f "$root/compose.yaml" --profile app logs --no-color --tail 40 "$service" 2>&1 | sed 's/^/     /' || true
  done
}

# artifact NAME FIELD: the field of the last artifact of that name of $events ("" if absent). An artifact reaches AG-UI as a
# vymalo.artifact activity whose content is {name, mimeType, text}; the coder's JSON is in `content.text`.
artifact() {
  jq -r --arg n "$1" --arg f "$2" '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.artifact" and .content.name == $n)
    | .content.text | fromjson? | .[$f] // empty] | last // empty' "$events"
}

# mock-github's view of the coder's credential: every call to /repos/... and /orgs/... carried it.
check_credentials() {
  github_find GET '/(repos|orgs)/.*' "$tmp/gets.json"
  github_find POST '/(repos|orgs)/.*' "$tmp/posts.json"
  jq -s '{requests: (.[0].requests + .[1].requests)}' "$tmp/gets.json" "$tmp/posts.json" > "$tmp/api-calls.json" 2>/dev/null || echo '{"requests":[]}' > "$tmp/api-calls.json"
  _n=$(jq -r '.requests | length' "$tmp/api-calls.json" 2>/dev/null || echo 0)
  case $github_auth in
    token)
      _want="Bearer ${MOCK_GITHUB_TOKEN-dev-github-token}"
      _wrong=$(jq -r --arg want "$_want" '[.requests[] | .headers | with_entries(.key |= ascii_downcase) | select(.authorization != $want)] | length' "$tmp/api-calls.json" 2>/dev/null || echo '?')
      if [ "$_n" -ge 1 ] && [ "$_wrong" = 0 ]; then ok "$1: all $_n call(s) to /repos/... and /orgs/... carried the token"; else bad "$1: $_wrong of $_n call(s) to /repos/... and /orgs/... did not carry '$_want'"; fi
      ;;
    app)
      _mints=$(count_of POST '/app/installations/67890/access_tokens')
      if [ "$_mints" != '?' ] && [ "$_mints" -ge 1 ]; then ok "$1: the coder traded a JWT for an installation token ($_mints POST /app/installations/67890/access_tokens)"; else bad "$1: mock-github saw $_mints POST /app/installations/67890/access_tokens, want at least 1 (is the stack started with -f dev/compose.github-app.yaml?)"; fi
      _wrong=$(jq -r '[.requests[] | .headers | with_entries(.key |= ascii_downcase) | select((.authorization // "") | startswith("Bearer ghs_mockinstallationtoken") | not)] | length' "$tmp/api-calls.json" 2>/dev/null || echo '?')
      if [ "$_n" -ge 1 ] && [ "$_wrong" = 0 ]; then ok "$1: all $_n call(s) to /repos/... and /orgs/... carried the installation token"; else bad "$1: $_wrong of $_n call(s) to /repos/... and /orgs/... did not carry 'Bearer ghs_mockinstallationtoken...'"; fi
      ;;
  esac
}

# check_published LABEL REPO_PATH: the thread ended `done` after the coder's work reached REPO_PATH on git-server: the artifacts, the gate,
# the CI card for that repository, the one pull request, the credential, and the branch on git-server. Sets branch, commit and
# checks_tree for the caller.
check_published() {
  _label=$1
  _path=$2
  _outcome_ok=0
  case $outcome in success) _outcome_ok=1 ;; esac
  if [ "$_outcome_ok" = 1 ]; then ok "$_label: the run stream ended with RUN_FINISHED (success)"; else bad "$_label: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"; fi
  if [ "$state" = "done" ]; then
    ok "$_label: the thread ended done"
  else
    bad "$_label: the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"
    why "$events"
    dump_logs
  fi
  branch=$(artifact branch branch)
  commit=$(artifact branch commit)
  checks_passed=$(artifact checks passed)
  checks_commit=$(artifact checks commit)
  checks_tree=$(artifact checks tree)
  pr_url=$(artifact pull_request url)
  pr_branch=$(artifact pull_request branch)
  if [ -n "$branch" ] && [ -n "$commit" ]; then ok "$_label: branch artifact: $branch at $(printf '%s' "$commit" | cut -c1-10)"; else bad "$_label: no branch artifact with a branch and a commit"; fi
  if [ "$checks_passed" = true ]; then ok "$_label: the last checks artifact passed"; else bad "$_label: the last checks artifact: passed is '${checks_passed:-absent}', want true"; fi
  if [ -n "$commit" ] && [ "$checks_commit" = "$commit" ]; then
    ok "$_label: the last checks artifact is bound to the pushed commit"
  else
    bad "$_label: the last checks artifact commit '$checks_commit' is not the pushed commit '$commit'"
  fi
  if [ -n "$pr_url" ] && [ -n "$branch" ] && [ "$pr_branch" = "$branch" ]; then ok "$_label: pull_request artifact for the pushed branch: $pr_url"; else bad "$_label: pull_request artifact: url '$pr_url', branch '$pr_branch', want one for '$branch'"; fi
  # The gate: the coder's own checks and CI, both on the pushed commit.
  _gate=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '(.job.gate // []) | join("+")' || true)
  if [ "$_gate" = ci+agent_checks ]; then ok "$_label: the job runs under the gates ci and agent_checks"; else bad "$_label: the job's gate is '${_gate:-none}', want ci+agent_checks"; fi
  _cards=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.check" and .content.source == "agent_checks")
    | "\(.content.status)@\(.content.commit)"] | last // empty' "$events" 2>/dev/null || true)
  if [ -n "$commit" ] && [ "$_cards" = "passed@$commit" ]; then ok "$_label: the agent_checks card passed on the pushed commit"; else bad "$_label: the last agent_checks card is '${_cards:-none}', want passed@${commit:-<commit>}"; fi
  _ci=$(jq -r --arg p "/$_path" '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ci")
    | "\(.content.name)=\(.content.conclusion)@\(.content.sha) \(.content.repository | endswith($p))"] | join(" | ")' "$events" 2>/dev/null || true)
  if [ -n "$commit" ] && [ "$_ci" = "mock-ci/build=success@$commit true" ]; then
    ok "$_label: one vymalo.ci card: mock-ci/build succeeded on the pushed commit, in $_path"
  else
    bad "$_label: the vymalo.ci cards are '${_ci:-none}' (name=conclusion@sha, is it $_path), want exactly one: mock-ci/build=success@${commit:-<commit>} true"
    dump_logs
  fi
  # The pull request, on the repository the work reached.
  _prs=$(count_of POST "/repos/$_path/pulls")
  if [ "$_prs" = 1 ]; then ok "$_label: mock-github saw exactly one POST /repos/$_path/pulls"; else bad "$_label: mock-github saw $_prs POST /repos/$_path/pulls, want exactly 1"; fi
  _head=$(jq -r '.requests[0].body | fromjson | .head // empty' "$tmp/found.json" 2>/dev/null || true)
  _base=$(jq -r '.requests[0].body | fromjson | .base // empty' "$tmp/found.json" 2>/dev/null || true)
  if [ -n "$branch" ] && [ "${_head##*:}" = "$branch" ] && [ "$_base" = main ]; then ok "$_label: the pull request is $branch into main"; else bad "$_label: the pull request is '$_head' into '$_base', want '$branch' into main"; fi
  check_credentials "$_label"
  # git-server: the branch is there, at the commit the artifact names.
  _remote=$(git ls-remote --heads "$gitserver/$_path.git" "refs/heads/$branch" 2>/dev/null || true)
  if [ -n "$branch" ] && [ "${_remote%%[[:space:]]*}" = "$commit" ] && [ -n "$commit" ]; then ok "$_label: git-server has $branch of $_path at the pushed commit"; else bad "$_label: git-server's $_path has '${_remote:-no such branch}', want $branch at '$commit'"; fi
  rm -rf "$tmp/clone"
  if [ -n "$branch" ] && git clone -q --depth 1 --branch "$branch" "$gitserver/$_path.git" "$tmp/clone" 2>"$tmp/clone.err"; then
    cloned=1
  else
    cloned=0
    bad "$_label: cannot clone $branch of $_path: $(head -c 300 "$tmp/clone.err" 2>/dev/null)"
  fi
}

# --- create-repo ---------------------------------------------------------------------------------------------
# create_repo yes|no
create_repo() {
  answer=$1
  label=create-repo
  [ "$answer" = yes ] || label=create-repo-no
  name=fib-$(printf '%x%x' "$(date +%s)" "$$")
  repo_path=scratch/$name
  thread=$(uuid)
  echo
  echo "== $label: no repository is named, the person asks for scratch/$name, and answers $answer (thread $thread)"
  reset_github "$label"

  # 1. The project is built and checked; the coder asks where to put it (a plain question: nothing to choose from).
  first_message "Write a fib.sh that prints the first 7 Fibonacci numbers. I'll give you a repo later. [mock:create-repo] $name" "$tmp/run1.json"
  stream "$tmp/run1.json" "$label, run 1"
  if [ "$outcome" = interrupt ] && [ "$state" = blocked ]; then
    ok "$label: the coder built the project and asks where to put it: the run ended RUN_FINISHED (interrupt), the thread is blocked"
  else
    bad "$label: after the first message the run ended '${outcome:-none}' and the thread is '${state:-unknown}', want interrupt and blocked"
    why "$events"
  fi
  case $said in *"where should it go"*) ok "$label: the coder's question is about where the project should go" ;; *) bad "$label: the coder's words do not ask where the project should go" ;; esac
  if [ "$(count_of POST '/repos/.*/pulls')" = 0 ] && [ "$(count_of POST '/orgs/.*/repos')" = 0 ]; then
    ok "$label: nothing has left the coder: no pull request, no repository created"
  else
    bad "$label: a pull request or a repository was made before the person named one"
  fi
  if [ "$(listed "$name.git")" = 0 ]; then ok "$label: git-server has not heard of $repo_path"; else bad "$label: git-server knows $repo_path before the person named it"; fi

  # 2. The person asks for a repository in words; the coder calls create_repository, whose question the tool writes.
  next_message "Create scratch/$name and put it there" "$tmp/run2.json"
  stream "$tmp/run2.json" "$label, run 2"
  if [ "$outcome" = interrupt ] && [ "$state" = blocked ]; then
    ok "$label: the coder asks for consent: the run ended RUN_FINISHED (interrupt), the thread is blocked"
  else
    bad "$label: after the request the run ended '${outcome:-none}' and the thread is '${state:-unknown}', want interrupt and blocked"
    why "$events"
  fi
  case $said in
    *"May I create the repository scratch/$name on"*"It will be private and empty"*) ok "$label: the question names the repository and says it will be private and empty" ;;
    *) bad "$label: the coder's words do not hold the question 'May I create the repository scratch/$name on ... It will be private and empty'" ;;
  esac
  expect_consent "$label" "Create scratch/$name"
  made=$(count_of POST /orgs/scratch/repos)
  if [ "$made" = 0 ]; then ok "$label: nothing was created while the person had not answered (no POST /orgs/scratch/repos)"; else bad "$label: mock-github saw $made POST /orgs/scratch/repos before the answer"; fi
  if [ "$(listed "$name.git")" = 0 ]; then ok "$label: git-server has not heard of $repo_path yet"; else bad "$label: git-server knows $repo_path before it was created"; fi

  # 3. The answer.
  answer_consent "$label, run 3" "$answer"
  made=$(count_of POST /orgs/scratch/repos)
  if [ -n "$choices_id" ]; then
    if [ "$(recorded_answer)" = "consent=$answer" ]; then ok "$label: the thread recorded the answer (a vymalo.action: consent=$answer)"; else bad "$label: the thread's vymalo.action says '$(recorded_answer)', want consent=$answer"; fi
  fi
  if [ "$answer" = no ]; then
    if [ "$state" = blocked ]; then ok "$label: the thread is blocked again: nothing was created"; else bad "$label: after the no the thread is '${state:-unknown}', want blocked"; fi
    case $said in *"I did not create the repository"*) ok "$label: the coder says it did not create the repository" ;; *) bad "$label: the coder does not say it did not create the repository" ;; esac
    if [ "$made" = 0 ]; then ok "$label: mock-github never saw a repository creation"; else bad "$label: mock-github saw $made POST /orgs/scratch/repos after the no"; fi
    if [ "$(listed "$name.git")" = 0 ]; then ok "$label: git-server never heard of $repo_path"; else bad "$label: git-server knows $repo_path although it was not created"; fi
    if [ "$(count_of POST '/repos/.*/pulls')" = 0 ]; then ok "$label: no pull request was opened"; else bad "$label: a pull request was opened after the no"; fi
    return 0
  fi

  # yes: exactly one creation, after the answer (it was 0 before), private, for that name, empty.
  if [ "$made" = 1 ]; then ok "$label: mock-github saw exactly one POST /orgs/scratch/repos, after the answer"; else bad "$label: mock-github saw $made POST /orgs/scratch/repos, want exactly 1"; fi
  github_find POST /orgs/scratch/repos "$tmp/creations.json"
  created_name=$(jq -r '.requests[0].body | fromjson | .name // empty' "$tmp/creations.json" 2>/dev/null || true)
  created_private=$(jq -r '.requests[0].body | fromjson | .private | tostring' "$tmp/creations.json" 2>/dev/null || true)
  created_init=$(jq -r '.requests[0].body | fromjson | .auto_init | tostring' "$tmp/creations.json" 2>/dev/null || true)
  if [ "$created_name" = "$name" ]; then ok "$label: the repository created is $name"; else bad "$label: the repository created is '$created_name', want '$name'"; fi
  if [ "$created_private" = true ]; then ok "$label: it was created private"; else bad "$label: private is '$created_private', want true"; fi
  if [ "$created_init" = false ]; then ok "$label: it was created empty (auto_init false)"; else bad "$label: auto_init is '$created_init', want false"; fi

  check_published "$label" "$repo_path"
  if [ "$cloned" = 1 ]; then
    content=$(cat "$tmp/clone/fib.sh" 2>/dev/null || echo '<missing>')
    if [ "$content" = 'echo 0 1 1 2 3 5 8' ] && [ "$(sh "$tmp/clone/fib.sh" 2>/dev/null)" = '0 1 1 2 3 5 8' ]; then
      ok "$label: fib.sh on the branch is the project's and prints the first 7 Fibonacci numbers"
    else
      bad "$label: fib.sh on the branch is '$content', want 'echo 0 1 1 2 3 5 8'"
    fi
    pushed_tree=$(git -C "$tmp/clone" rev-parse 'HEAD^{tree}' 2>/dev/null || true)
    if [ -n "$pushed_tree" ] && [ "$pushed_tree" = "$checks_tree" ]; then ok "$label: the tree that was checked in the scratch project is the tree that was pushed"; else bad "$label: the pushed tree is '$pushed_tree', the checks ran on '$checks_tree'"; fi
  fi
  rm -rf "$tmp/main"
  if git clone -q --branch main "$gitserver/$repo_path.git" "$tmp/main" 2>"$tmp/main.err"; then
    root_tree=$(git -C "$tmp/main" rev-parse 'HEAD^{tree}' 2>/dev/null || true)
    n_commits=$(git -C "$tmp/main" rev-list --count HEAD 2>/dev/null || echo '?')
    if [ "$root_tree" = 4b825dc642cb6eb9a060e54bf8d69288fbee4904 ] && [ "$n_commits" = 1 ]; then ok "$label: main is the one empty commit the coder gave the new repository"; else bad "$label: main has $n_commits commit(s) and the tree '$root_tree', want the one empty commit"; fi
  else
    bad "$label: cannot clone main of $repo_path: $(head -c 300 "$tmp/main.err")"
  fi
  # The credential never went through the orchestrator: it is not in the thread's log.
  if api GET "/api/threads/$thread/export" > "$tmp/export.json" 2>"$tmp/err"; then
    if grep -Eq 'dev-github-token|ghs_mockinstallationtoken|PRIVATE KEY' "$tmp/export.json"; then
      bad "$label: the thread's export holds a GitHub credential"
    else
      ok "$label: no GitHub credential is in the thread's export"
    fi
  else
    bad "$label: GET /api/threads/$thread/export: $(head -c 300 "$tmp/err")"
  fi
}

# --- second-repo ---------------------------------------------------------------------------------------------
# second_repo yes|no
second_repo() {
  answer=$1
  label=second-repo
  [ "$answer" = yes ] || label=second-repo-no
  repo_path=local/sandbox
  repo_url=$repo_base/$repo_path.git
  thread=$(uuid)
  echo
  echo "== $label: local/library joins the workspace only if the person says yes, and they say $answer (thread $thread)"
  reset_github "$label"
  library_before=$(library_requests)
  if [ "$answer" = yes ]; then
    # A base of this run's own, as dev/coder-e2e.sh does: git dates have one-second resolution, and a commit belongs to the first job that
    # pushed it, so a push identical to an earlier run's would never hear its CI report. An empty commit naming the thread on main of the
    # sandbox makes this run's commit its own; the tree is unchanged.
    rm -rf "$tmp/base"
    if git clone -q --depth 1 --branch main "$gitserver/$repo_path.git" "$tmp/base" 2>"$tmp/base.err" &&
      git -C "$tmp/base" -c user.name=workspace-e2e -c user.email=workspace-e2e@example.invalid -c commit.gpgsign=false \
        commit -q --allow-empty -m "workspace-e2e base for thread $thread" 2>>"$tmp/base.err" &&
      git -C "$tmp/base" push -q origin HEAD:main 2>>"$tmp/base.err"; then
      ok "$label: main of $repo_path is at a base of this run's own"
    else
      bad "$label: cannot give this run a base of its own on main: $(head -c 300 "$tmp/base.err")"
    fi
  fi

  # 1. The coder prepares the sandbox and asks whether it may add the library.
  first_message "In $repo_url (base branch main), put our shared greeting into hello.txt. [mock:second-repo]" "$tmp/run1.json"
  stream "$tmp/run1.json" "$label, run 1"
  if [ "$outcome" = interrupt ] && [ "$state" = blocked ]; then
    ok "$label: the coder asks whether it may add local/library: the run ended RUN_FINISHED (interrupt), the thread is blocked"
  else
    bad "$label: after the first message the run ended '${outcome:-none}' and the thread is '${state:-unknown}', want interrupt and blocked"
    why "$events"
  fi
  case $said in *"May I add the repository local/library"*) ok "$label: the question names local/library" ;; *) bad "$label: the coder's words do not hold the question 'May I add the repository local/library'" ;; esac
  case $said in *'The agent says why: "the shared greeting lives in greeting.txt there"'*) ok "$label: the question quotes the reason" ;; *) bad "$label: the question does not quote the reason" ;; esac
  expect_consent "$label" "Yes, add local/library"
  if [ "$(count_of POST '/repos/.*/pulls')" = 0 ]; then ok "$label: no pull request was opened while the coder waits for the answer"; else bad "$label: a pull request was opened before the person answered"; fi
  library_waiting=$(library_requests)
  if [ "$library_before" = '?' ]; then
    echo "skip $label: git-server's log is not readable here (set GIT_SERVER_LOGS): not checking when local/library was asked for"
  elif [ "$library_waiting" = "$library_before" ]; then
    ok "$label: git-server was not asked for local/library before the person answered"
  else
    bad "$label: git-server was asked for local/library $((library_waiting - library_before)) time(s) before the person answered"
  fi

  # 2. The answer.
  answer_consent "$label, run 2" "$answer"
  library_after=$(library_requests)
  if [ -n "$choices_id" ]; then
    if [ "$(recorded_answer)" = "consent=$answer" ]; then ok "$label: the thread recorded the answer (a vymalo.action: consent=$answer)"; else bad "$label: the thread's vymalo.action says '$(recorded_answer)', want consent=$answer"; fi
  fi
  if [ "$answer" = no ]; then
    if [ "$state" = blocked ]; then ok "$label: the thread is blocked again: the coder could not use the library"; else bad "$label: after the no the thread is '${state:-unknown}', want blocked"; fi
    case $said in *"I could not add the library repository"*) ok "$label: the coder tells the person it could not add the library" ;; *) bad "$label: the coder does not say it could not add the library" ;; esac
    if [ "$library_before" = '?' ]; then
      echo "skip $label: git-server's log is not readable here: not checking that local/library was never asked for"
    elif [ "$library_after" = "$library_before" ]; then
      ok "$label: git-server was never asked for local/library"
    else
      bad "$label: git-server was asked for local/library $((library_after - library_before)) time(s) after the no"
    fi
    if [ "$(count_of POST '/repos/.*/pulls')" = 0 ]; then ok "$label: no pull request was opened"; else bad "$label: a pull request was opened after the no"; fi
    return 0
  fi

  check_published "$label" "$repo_path"
  if [ "$cloned" = 1 ]; then
    content=$(cat "$tmp/clone/hello.txt" 2>/dev/null || echo '<missing>')
    if [ "$content" = 'hello from library' ]; then ok "$label: hello.txt on the branch is the library's greeting"; else bad "$label: hello.txt on the branch is '$content', want 'hello from library'"; fi
  fi
  if [ "$library_before" = '?' ]; then
    echo "skip $label: git-server's log is not readable here: not checking that local/library was asked for after the answer"
  elif [ "$library_after" -gt "$library_before" ]; then
    ok "$label: git-server was asked for local/library after the yes ($((library_after - library_before)) request(s))"
  else
    bad "$label: git-server was never asked for local/library, although the person said yes"
  fi
}

for s in $scenarios; do
  case $s in
    create-repo) create_repo yes ;;
    create-repo-no) create_repo no ;;
    second-repo) second_repo yes ;;
    second-repo-no) second_repo no ;;
  esac
done

echo
finish
