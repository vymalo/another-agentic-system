#!/usr/bin/env sh
# System-level test of the coder's work environments (MVP slice 7b, ADR 0028; adam-rs ADR 0010): a repository's own
# devcontainer is the environment its run works in, a repository without one gets a default image, a runtime that is not
# there and a devcontainer.json that cannot be used are said and never hidden, what the run made is cleaned up, and the
# Podman service that hosts all this is given no more than it needs. The whole chain goes through the edge, the
# orchestrator and the coder, as a person's chat does (AG-UI); the model, GitHub, git and CI are the stack's mocks.
#
#   dev/devcontainer-e2e.sh                                  # the four scenarios below, and the privileges
#   SCENARIOS="default-env" dev/devcontainer-e2e.sh          # only these (devcontainer default-env broken-env no-runtime)
#   PRELOAD_FROM_DOCKER=1 dev/devcontainer-e2e.sh            # where containers have no internet (see below)
#
# Start the stack WITH the override first (the coder image is about 2.9 GB, linux/amd64 only; the service is built on Podman's own
# image, about 250 MB, and pulls a 367 MiB base image on the first run, the one the fixture's image is built on too):
#
#   docker compose -f compose.yaml -f dev/compose.devcontainer.yaml --profile app up -d --build --wait
#
# (On a stack that is already up without it, `... up -d --no-build --wait podman coder mock-ci` recreates the three that change: the
# service itself, the coder, which then uses it, and mock-ci, which then watches local/devbox, where the gate waits for CI.)
#
# An Ubuntu 24.04 host (a CI runner, a desktop) restricts unprivileged user namespaces, which a rootless Podman needs:
# `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` first (dev/README.md, "Devcontainers"). Without it the service
# starts but cannot run a container, the coder falls back to its own, and these scenarios FAIL saying so.
#
# `dev/e2e-all.sh` does not run this script: it needs the override (another shape of the stack, like `split`), and the scripts of
# e2e-all restart the coder without it. CI runs it in .github/workflows/coder-e2e.yml, after the sysctl.
#
# The script speaks AG-UI, as dev/workspace-e2e.sh does, with one new thread per scenario on the agent `coder`. The coder's model is
# `mock-coder`: a task that carries `[mock:devcontainer]`, `[mock:default-env]`, `[mock:broken-env]` or `[mock:no-runtime]` selects the
# script (dev/coder/wiremock/mock-openai/mappings/coder-script.json, vendored from adam-rs, and OpenCode's `[mock:oc-devbox]` in
# opencode-script.json). What a tool returned is read from the model's own request journal (the history of the next request holds
# it), the steps the environment reports are `vymalo.step` activities of the thread's frames, and what Podman lists is read from the
# service itself (`docker compose exec podman podman --remote ps`). The fixtures are the seeded repositories of git-server:
# `local/devbox` (its devcontainer has `devbox-tool`, which nothing else does), `local/devbox-broken` (every part of its
# devcontainer.json is hostile) and `local/sandbox` (no devcontainer).
#
# privileges (always, first): `docker compose config` shows no `privileged`, no `cap_add` and no `devices` on the `podman` service
# and no service mounts a Docker socket, and the same holds of the container that runs (`docker inspect`).
#
# devcontainer: `local/devbox`. The thread ends `done` and the run stream RUN_FINISHED (success); the step "Building the environment
# from .devcontainer/devcontainer.json (local/devbox)" ends `completed` (in the frames and in the log's `agent_step` events);
# `run_command` finds `devbox-tool` (its result says `devbox-tool 1.0 (from the devcontainer)`), `env` there shows none of the coder's
# secrets (GITHUB_TOKEN, DATABASE_URL, A2A_BEARER_TOKENS, MODEL_API_KEY and their values), the checks artifact says
# `environment {kind: devcontainer, source: .devcontainer/devcontainer.json}`, OpenCode (a sub-agent step) ran its own bash
# command there (`tool.txt` on the pushed branch says "from the devcontainer"); the gate is `ci+agent_checks`, the agent_checks
# card passed on the pushed commit and `mock-ci/build` succeeded on it, and mock-github saw exactly one pull request. Podman lists a
# container with the run's label while the run lasts (`adam.vymalo.com/run`), and none within PODMAN_WAIT_SECS of its end: the
# janitor released it (WORKSPACE_SWEEP_SECS=10).
#
# default-env: `local/sandbox`, which has no devcontainer. The step "Using the default environment (<image>)" ends `completed`,
# the probe `test -d /opt/flutter && echo coder-env || echo devcontainer-env` prints `devcontainer-env` (only the coder's own
# image has /opt/flutter), the checks artifact says `environment.kind` is devcontainer with no source file, the gate and the pull
# request hold as above (`hello.txt` on the branch), and Podman lists none with the run's label afterwards.
#
# broken-env: `local/devbox-broken`. The step "Building the environment from .devcontainer/devcontainer.json (local/devbox-broken)"
# ends `failed` and its detail names `privileged`; the result of the first command names the file, the key, `ask_user` and
# `rebuild_environment` (there is no silent fallback: the person decides); the run ends RUN_FINISHED (interrupt) and the thread is
# `blocked` on the coder's question; no pull request. The file's hostile parts did nothing: /work/INIT-RAN (its `initializeCommand`)
# does not exist in the coder, the refused file made no container at all, and the value of the coder's GITHUB_TOKEN, which its
# `${localEnv:GITHUB_TOKEN}` asks for, is in no container of the service and not in the thread's export.
#
# no-runtime: `local/devbox`, with the Podman service stopped (and started again at the end). A step says "The container runtime is
# not reachable: commands run in the coder's own environment" (`completed`), `devbox-tool` is reported as a missing tool by the
# result of the first command, the coder asks the person what to do, and no pull request is opened. The script waits out the coder's
# 30-second cache of a probe first: a run that began before would still find the service.
#
# It prints one ok or FAIL line per check and exits 1 if any failed.
#
# Environment (defaults match compose.yaml on one machine; run it from anywhere, it changes to the repository's root):
#   BASE_URL         http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   AUTH_EMAIL       dev@example.com, the user: a token of the mock issuer (dev/auth-header.sh)
#   MOCK_GITHUB_URL  http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}   (its journal is emptied at the start of each scenario)
#   MOCK_OPENAI_URL  http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}   (the coder's model; its journal is emptied at the start of each)
#   GIT_SERVER_URL   http://127.0.0.1:${GIT_SERVER_PORT:-8093}    (from the host)
#   TIMEOUT          900    seconds to wait for a run to end (the first run pulls and builds images)
#   SCENARIOS        devcontainer default-env broken-env no-runtime
#   COMPOSE_CMD      docker compose -f compose.yaml -f dev/compose.devcontainer.yaml --profile app   how to reach the stack's `podman`,
#                    `coder` and its configuration
#   PRELOAD_FROM_DOCKER  unset  1 = load the default image into the Podman service from the host's Docker (docker save | podman load),
#                    for where containers have no direct internet. *Unverified*: the digest of a loaded image may differ from the pin
#                    (then the service pulls it)
#   PODMAN_WAIT_SECS 90     how long to wait for the Podman service to hold no container of the run after it ended
#   DEVCONTAINER_DEFAULT_IMAGE  the image the stack's coder was given (the default of the override), only to preload it
#
# Needs curl, jq, git and docker (compose v2). Verified by CI only, in .github/workflows/coder-e2e.yml.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
cd "$root"

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
email=${AUTH_EMAIL:-dev@example.com}
# The API wants a bearer token of the mock issuer, not a header (ADR 0033): dev/auth-header.sh prints the header line.
id_header=$(sh "$here/auth-header.sh" "$email")
github=${MOCK_GITHUB_URL:-http://127.0.0.1:${MOCK_GITHUB_PORT:-8092}}
github=${github%/}
openai=${MOCK_OPENAI_URL:-http://127.0.0.1:${MOCK_OPENAI_PORT:-8091}}
openai=${openai%/}
gitserver=${GIT_SERVER_URL:-http://127.0.0.1:${GIT_SERVER_PORT:-8093}}
gitserver=${gitserver%/}
repo_base=http://git-server:8080 # where the coder, inside the compose network, finds git-server
timeout=${TIMEOUT:-900}
wait_secs=${PODMAN_WAIT_SECS:-90}
scenarios=${SCENARIOS:-devcontainer default-env broken-env no-runtime}
compose=${COMPOSE_CMD:-docker compose -f compose.yaml -f dev/compose.devcontainer.yaml --profile app}
default_image=${DEVCONTAINER_DEFAULT_IMAGE:-mcr.microsoft.com/devcontainers/base@sha256:1f851004adcd3dff3776b4a1da86727cf52280b0fdc9d5b0568c9c7d74274286}
for s in $scenarios; do
  case $s in
    devcontainer | default-env | broken-env | no-runtime) ;;
    *) echo "unknown scenario '$s'; choose from: devcontainer default-env broken-env no-runtime" >&2; exit 2 ;;
  esac
done

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
note() { echo "NOTE $1"; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "devcontainer e2e passed"; else echo "devcontainer e2e FAILED"; exit 1; fi
}

# dc ARGS: `docker compose` of the stack with the override (COMPOSE_CMD is a command line, hence the unquoted expansion).
dc() {
  # shellcheck disable=SC2086
  $compose "$@"
}

tmp=$(mktemp -d)
watcher=
podman_stopped=0
cleanup() {
  if [ -n "$watcher" ]; then kill "$watcher" 2>/dev/null || true; fi
  if [ "$podman_stopped" = 1 ]; then
    dc up -d --wait podman >/dev/null 2>&1 || echo "could not start the Podman service again: start it before the next scenario" >&2
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

api() { # api METHOD PATH: the body on stdout, non-zero when the status is not 2xx (the resource API)
  curl --fail-with-body -sS --max-time 60 -X "$1" "$base$2" -H "$id_header"
}

uuid() { cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen | tr 'A-F' 'a-f'; }

sse_events() { # sse_events FILE: the AG-UI events of a saved SSE response, one JSON per line
  sed -n 's/^data: *//p' "$1"
}

# dump_logs: the tail of the logs that say what went wrong, when docker is here.
dump_logs() {
  command -v docker >/dev/null 2>&1 || return 0
  for service in coder podman; do
    echo "     --- docker compose logs --tail 60 $service"
    dc logs --no-color --tail 60 "$service" 2>&1 | sed 's/^/     /' || true
  done
}

# --- the stack ----------------------------------------------------------------------------------------------------
# podman_cli ARGS: Podman's remote client, in the service's own container, on its own socket (the service runs as uid 10001, the
# socket's owner).
podman_cli() {
  dc exec -T podman podman --remote --url unix:///run/podman/podman.sock "$@"
}

# podman_ps: the ids of the containers that carry a run's label, as the service lists them (stopped ones too).
podman_ps() {
  podman_cli ps -a --filter label=adam.vymalo.com/run --format '{{.ID}}' 2>/dev/null || true
}

require_stack() {
  if ! dc ps --status running --services 2>/dev/null | grep -qx podman; then
    echo "FAIL the Podman service is not running in this stack: start it with -f compose.yaml -f dev/compose.devcontainer.yaml (COMPOSE_CMD='$compose')"
    exit 1
  fi
  _rt=$(dc exec -T coder printenv DEVCONTAINER_RUNTIME 2>/dev/null || true)
  if [ "$_rt" != podman ]; then
    echo "FAIL the coder runs with DEVCONTAINER_RUNTIME='${_rt:-unset}', want podman: recreate it with the override (docker compose -f compose.yaml -f dev/compose.devcontainer.yaml --profile app up -d --no-build --wait podman coder mock-ci); a script that restarts the coder without the override (dev/agent-folder-e2e.sh) undoes it"
    exit 1
  fi
  # The gate waits for CI on the pushed commit of local/devbox: the override makes mock-ci watch it.
  if ! dc exec -T mock-ci printenv MOCK_CI_REPOS 2>/dev/null | grep -q 'local/devbox'; then
    echo "FAIL mock-ci does not watch local/devbox, so a job on it would wait for CI for ever: recreate it with the override (docker compose -f compose.yaml -f dev/compose.devcontainer.yaml --profile app up -d --no-build --wait podman coder mock-ci)"
    exit 1
  fi
}

# preload: where containers have no direct internet, the default image goes from the host's Docker into the Podman service.
preload() {
  [ "${PRELOAD_FROM_DOCKER:-}" = 1 ] || return 0
  docker image inspect "$default_image" >/dev/null 2>&1 || docker pull -q "$default_image" >/dev/null 2>&1 || true
  if docker save "$default_image" 2>/dev/null | podman_cli load >/dev/null 2>&1; then
    ok "the default image was loaded into the Podman service from the host's Docker"
  else
    note "the default image could not be preloaded (PRELOAD_FROM_DOCKER=1): the service will pull it"
  fi
}

# --- the mocks ----------------------------------------------------------------------------------------------------
reset_journals() { # reset_journals LABEL: mock-github's and the coder's model's request journals
  for _m in "$github" "$openai"; do
    _c=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -X DELETE "$_m/__admin/requests" || true)
    if [ "$_c" = 200 ]; then ok "$1: journal reset: $_m"; else bad "$1: journal reset: $_m answered HTTP $_c"; fi
  done
}

# tool_result_has CALL_ID TEXT: how many requests the coder's model got whose history holds the result of that tool call with TEXT
# in it (`?` when the journal cannot be read). The journal is emptied at the start of each scenario.
tool_result_has() {
  _expr="\$.messages[?(@.tool_call_id == '$1')].content"
  curl -s --max-time 30 -X POST "$openai/__admin/requests/count" -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg e "$_expr" --arg t "$2" '{method: "POST", urlPathPattern: "(/v1)?/chat/completions", bodyPatterns: [{matchesJsonPath: {expression: $e, contains: $t}}]}')" \
    | jq -r '.count' 2>/dev/null || echo '?'
}

# result_says LABEL CALL_ID TEXT DESCRIPTION: a check that the result of the tool call holds TEXT.
result_says() {
  _n=$(tool_result_has "$2" "$3")
  if [ "$_n" != '?' ] && [ "$_n" -ge 1 ]; then ok "$1: $4"; else bad "$1: $4 is not so (the model's history holds no such result of $2: $_n requests)"; fi
}

# result_lacks LABEL CALL_ID TEXT: the result of the tool call does not hold TEXT.
result_lacks() {
  _n=$(tool_result_has "$2" "$3")
  if [ "$_n" = 0 ]; then ok "$1: the result of $2 has no $3"; else bad "$1: the result of $2 shows $3 ($_n requests)"; fi
}

pulls_of() { # pulls_of REPO_PATH: how many POST /repos/<repo>/pulls mock-github saw
  curl -s --max-time 30 -X POST "$github/__admin/requests/find" -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg p "/repos/$1/pulls" '{method: "POST", urlPathPattern: $p}')" > "$tmp/found.json" || true
  jq -r '.requests | length' "$tmp/found.json" 2>/dev/null || echo '?'
}

# own_base REPO_PATH THREAD: an empty commit naming the thread on main of the repository, so that the commit this run pushes is its
# own (git dates have one-second resolution, and a commit belongs to the first job that pushed it: a push identical to an earlier
# run's would never hear its CI report, as dev/coder-e2e.sh explains). The tree, and so check.sh, is unchanged.
own_base() {
  rm -rf "$tmp/base"
  if git clone -q --depth 1 --branch main "$gitserver/$1.git" "$tmp/base" 2>"$tmp/base.err" &&
    git -C "$tmp/base" -c user.name=devcontainer-e2e -c user.email=devcontainer-e2e@example.invalid -c commit.gpgsign=false \
      commit -q --allow-empty -m "devcontainer-e2e base for thread $2" 2>>"$tmp/base.err" &&
    git -C "$tmp/base" push -q origin HEAD:main 2>>"$tmp/base.err"; then
    ok "main of $1 is at a base of this run's own"
  else
    bad "cannot give this run a base of its own on main of $1: $(head -c 300 "$tmp/base.err")"
  fi
}

# --- one run ------------------------------------------------------------------------------------------------------
thread=
# message TEXT FILE: the RunAgentInput of the thread's first (only) run: one user message.
message() {
  jq -nc --arg thread "$thread" --arg run "$(uuid)" --arg msg "$(uuid)" --arg text "$1" '{
    threadId: $thread, runId: $run, state: {}, tools: [], context: [],
    messages: [{id: $msg, role: "user", content: $text}], forwardedProps: {}}' > "$2"
}

# stream INPUT_FILE LABEL: POST the RunAgentInput and wait for the run to end, then for the thread to settle. Sets
#   outcome   how the run stream ended (success, interrupt, error: <code>, or empty)
#   state     the state the thread ended in
#   said      the words the coder spoke in this run (a live message as a client keeps it)
#   events    the file holding every frame of the thread so far (a JSON array)
stream() {
  _deadline=$(( $(date +%s) + timeout ))
  _code=$(curl -sS -N --max-time "$timeout" -o "$tmp/run.sse" -w '%{http_code}' -X POST \
    "$base/agui/agents/coder" -H "$id_header" \
    -H 'content-type: application/json' -H 'accept: text/event-stream' --data-binary "@$1" 2>"$tmp/err" || true)
  events=$tmp/events.json
  if [ "$_code" != 200 ]; then
    bad "$2: POST /agui/agents/coder answered HTTP ${_code:-none}: $(head -c 300 "$tmp/err") $(head -c 400 "$tmp/run.sse" 2>/dev/null)"
    outcome=
    state=
    said=
    echo '[]' > "$events"
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

# artifact_json NAME FILTER: a jq filter (`.passed`, `.environment.kind`) of the last artifact of that name of $events ("" if absent).
# An artifact reaches AG-UI as a vymalo.artifact activity whose content is {name, mimeType, text}: the coder's JSON is in `content.text`.
artifact_json() {
  jq -r --arg n "$1" "[.[] | select(.type == \"ACTIVITY_SNAPSHOT\" and .activityType == \"vymalo.artifact\" and .content.name == \$n)
    | .content.text | fromjson? | $2 // empty] | last // empty" "$events" 2>/dev/null || true
}

# step_field LABEL_PREFIX FIELD: the field (state, detail, kind) of the LAST snapshot of the step whose label starts with the prefix.
# The replay holds one snapshot per report of a step, the start first, the end last (docs/api/agui.md, "Nested steps").
step_field() {
  jq -r --arg p "$1" --arg f "$2" '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step" and ((.content.label // "") | startswith($p)))]
    | last | (.content[$f] // empty)' "$events" 2>/dev/null || true
}

# step_ends LABEL STEP_LABEL_PREFIX STATE: a check that the (last snapshot of the) step ended in that state.
step_ends() {
  _s=$(step_field "$2" state)
  if [ "$_s" = "$3" ]; then ok "$1: the step '$2' ended $3"; else bad "$1: the step '$2' is '${_s:-absent}', want $3 (steps: $(jq -c '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.step") | "\(.content.label): \(.content.state)"] | unique' "$events" 2>/dev/null | head -c 600))"; fi
}

# export_thread: the thread's export, in $tmp/export.json (the whole log, for what a developer is sent).
export_thread() {
  if ! BASE_URL=$base AUTH_EMAIL=$email sh "$here/export-thread.sh" "$thread" "$tmp/export.json" 2>"$tmp/export.err"; then
    bad "dev/export-thread.sh failed: $(head -c 300 "$tmp/export.err")"
    rm -f "$tmp/export.json"
  fi
}

# no_pull_request LABEL REPO_PATH
no_pull_request() {
  _p=$(pulls_of "$2")
  if [ "$_p" = 0 ]; then ok "$1: no pull request was opened"; else bad "$1: $_p pull request(s) were opened on $2, want none"; fi
}

# check_published LABEL REPO_PATH: the thread ended `done` after the coder's work reached REPO_PATH: the artifacts, the gate, the CI
# card for that repository, the one pull request and the branch on git-server. Sets branch and commit, and `cloned` (the branch is in
# $tmp/clone).
check_published() {
  _l=$1
  _path=$2
  if [ "$outcome" = success ]; then ok "$_l: the run stream ended with RUN_FINISHED (success)"; else bad "$_l: the run stream ended with '${outcome:-no terminal event}', want RUN_FINISHED (success)"; fi
  if [ "$state" = "done" ]; then
    ok "$_l: the thread ended done"
  else
    bad "$_l: the thread ended '${state:-unknown}' (after at most ${timeout}s), want done"
    why "$events"
    dump_logs
  fi
  branch=$(artifact_json branch .branch)
  commit=$(artifact_json branch .commit)
  if [ -n "$branch" ] && [ -n "$commit" ]; then ok "$_l: branch artifact: $branch at $(printf '%s' "$commit" | cut -c1-10)"; else bad "$_l: no branch artifact with a branch and a commit"; fi
  _passed=$(artifact_json checks '.passed | tostring')
  _bound=$(artifact_json checks .commit)
  if [ "$_passed" = true ]; then ok "$_l: the last checks artifact passed"; else bad "$_l: the last checks artifact: passed is '${_passed:-absent}', want true"; fi
  if [ -n "$commit" ] && [ "$_bound" = "$commit" ]; then ok "$_l: the last checks artifact is bound to the pushed commit"; else bad "$_l: the last checks artifact commit '$_bound' is not the pushed commit '$commit'"; fi
  _pr=$(artifact_json pull_request .url)
  _prb=$(artifact_json pull_request .branch)
  if [ -n "$_pr" ] && [ -n "$branch" ] && [ "$_prb" = "$branch" ]; then ok "$_l: pull_request artifact for the pushed branch: $_pr"; else bad "$_l: pull_request artifact: url '$_pr', branch '$_prb', want one for '$branch'"; fi
  # The gate: the coder's own checks and CI, both on the pushed commit.
  _gate=$(api GET "/api/threads/$thread" 2>/dev/null | jq -r '(.job.gate // []) | join("+")' || true)
  if [ "$_gate" = ci+agent_checks ]; then ok "$_l: the job runs under the gates ci and agent_checks"; else bad "$_l: the job's gate is '${_gate:-none}', want ci+agent_checks"; fi
  _cards=$(jq -r '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.check" and .content.source == "agent_checks")
    | "\(.content.status)@\(.content.commit)"] | last // empty' "$events" 2>/dev/null || true)
  if [ -n "$commit" ] && [ "$_cards" = "passed@$commit" ]; then ok "$_l: the agent_checks card passed on the pushed commit"; else bad "$_l: the last agent_checks card is '${_cards:-none}', want passed@${commit:-<commit>}"; fi
  _ci=$(jq -r --arg p "/$_path" '[.[] | select(.type == "ACTIVITY_SNAPSHOT" and .activityType == "vymalo.ci")
    | "\(.content.name)=\(.content.conclusion)@\(.content.sha) \(.content.repository | endswith($p))"] | join(" | ")' "$events" 2>/dev/null || true)
  if [ -n "$commit" ] && [ "$_ci" = "mock-ci/build=success@$commit true" ]; then
    ok "$_l: one vymalo.ci card: mock-ci/build succeeded on the pushed commit, in $_path"
  else
    bad "$_l: the vymalo.ci cards are '${_ci:-none}' (name=conclusion@sha, is it $_path), want exactly one: mock-ci/build=success@${commit:-<commit>} true (does mock-ci watch $_path? dev/compose.devcontainer.yaml sets MOCK_CI_REPOS)"
    dump_logs
  fi
  _prs=$(pulls_of "$_path")
  if [ "$_prs" = 1 ]; then ok "$_l: mock-github saw exactly one POST /repos/$_path/pulls"; else bad "$_l: mock-github saw $_prs POST /repos/$_path/pulls, want exactly 1"; fi
  _head=$(jq -r '.requests[0].body | fromjson | .head // empty' "$tmp/found.json" 2>/dev/null || true)
  _base=$(jq -r '.requests[0].body | fromjson | .base // empty' "$tmp/found.json" 2>/dev/null || true)
  if [ -n "$branch" ] && [ "${_head##*:}" = "$branch" ] && [ "$_base" = main ]; then ok "$_l: the pull request is $branch into main"; else bad "$_l: the pull request is '$_head' into '$_base', want '$branch' into main"; fi
  _remote=$(git ls-remote --heads "$gitserver/$_path.git" "refs/heads/$branch" 2>/dev/null || true)
  if [ -n "$branch" ] && [ -n "$commit" ] && [ "${_remote%%[[:space:]]*}" = "$commit" ]; then ok "$_l: git-server has $branch of $_path at the pushed commit"; else bad "$_l: git-server's $_path has '${_remote:-no such branch}', want $branch at '$commit'"; fi
  rm -rf "$tmp/clone"
  if [ -n "$branch" ] && git clone -q --depth 1 --branch "$branch" "$gitserver/$_path.git" "$tmp/clone" 2>"$tmp/clone.err"; then
    cloned=1
  else
    cloned=0
    bad "$_l: cannot clone $branch of $_path: $(head -c 300 "$tmp/clone.err" 2>/dev/null)"
  fi
}

# teardown LABEL: the run is over; the janitor (WORKSPACE_SWEEP_SECS=10) releases its environment, and Podman then lists no
# container with the run's label.
teardown() {
  _waited=0
  _left=$(podman_ps)
  while [ -n "$_left" ] && [ "$_waited" -lt "$wait_secs" ]; do
    sleep 3
    _waited=$((_waited + 3))
    _left=$(podman_ps)
  done
  if [ -z "$_left" ]; then
    ok "$1: after the run ended and the sweep, \`podman ps -a --filter label=adam.vymalo.com/run\` is empty"
  else
    bad "$1: Podman still lists a container with the run's label ${wait_secs}s after the run ended: $_left"
  fi
  # The images the CLI built for the run (`vsc-*`) go with it; a pulled base image stays. Said, not asserted: adam-rs's own end to
  # end asserts the containers only.
  _imgs=$(podman_cli images --format '{{.Repository}}' 2>/dev/null | grep -c '^localhost/vsc-' || true)
  note "$1: the service holds $_imgs built vsc-* image(s) after the sweep (the janitor removes a run's own)"
}

# --- privileges ---------------------------------------------------------------------------------------------------
privileges() {
  echo
  echo "== privileges: what the Podman service is given"
  if dc config --format json > "$tmp/compose.json" 2>"$tmp/compose.err"; then
    _flags=$(jq -r '.services.podman | [(if .privileged == true then "privileged" else empty end), (if ((.cap_add // []) | length) > 0 then "cap_add" else empty end), (if ((.devices // []) | length) > 0 then "devices" else empty end)] | join(",")' "$tmp/compose.json")
    if [ -z "$_flags" ]; then ok "privileges: the compose model of the Podman service has no privileged, no cap_add and no devices"; else bad "privileges: the Podman service has: $_flags"; fi
    _sock=$(jq -r '[.services | to_entries[] | .key as $s | (.value.volumes // [])[] | select((.source // "") | test("docker\\.sock")) | "\($s): \(.source)"] | join(", ")' "$tmp/compose.json")
    if [ -z "$_sock" ]; then ok "privileges: no service mounts a Docker socket"; else bad "privileges: a Docker socket is mounted: $_sock"; fi
    _opts=$(jq -r '.services.podman.security_opt // [] | map(sub("=.*"; "")) | sort | join(",")' "$tmp/compose.json")
    if [ "$_opts" = apparmor,seccomp,systempaths ]; then ok "privileges: the service's security options are seccomp (a profile file), systempaths and apparmor, and nothing else"; else bad "privileges: the service's security options are '$_opts', want apparmor,seccomp,systempaths"; fi
  else
    bad "privileges: cannot read the stack's compose model ($compose config): $(head -c 300 "$tmp/compose.err")"
  fi
  # The container that runs is the proof: what the engine was asked to start.
  _id=$(dc ps -q podman 2>/dev/null | head -n 1 || true)
  if [ -n "$_id" ] && docker inspect "$_id" > "$tmp/inspect.json" 2>/dev/null; then
    _run=$(jq -r '.[0].HostConfig | [(if .Privileged == true then "privileged" else empty end), (if ((.CapAdd // []) | length) > 0 then "CapAdd" else empty end), (if ((.Devices // []) | length) > 0 then "devices" else empty end)] | join(",")' "$tmp/inspect.json")
    if [ -z "$_run" ]; then ok "privileges: the running Podman container is not privileged and has no CapAdd and no devices"; else bad "privileges: the running Podman container has: $_run"; fi
    _user=$(jq -r '.[0].Config.User' "$tmp/inspect.json")
    if [ "$_user" = 10001:10001 ]; then ok "privileges: it runs as 10001:10001, the coder's uid"; else bad "privileges: it runs as '$_user', want 10001:10001"; fi
    _mounts=$(jq -r '[.[0].Mounts[] | .Source | select(test("docker\\.sock"))] | length' "$tmp/inspect.json")
    if [ "$_mounts" = 0 ]; then ok "privileges: the running Podman container mounts no Docker socket"; else bad "privileges: the running Podman container mounts a Docker socket"; fi
  else
    note "privileges: the running container could not be inspected (docker inspect): only the compose model was checked"
  fi
}

# --- devcontainer -------------------------------------------------------------------------------------------------
scenario_devcontainer() {
  label=devcontainer
  repo_path=local/devbox
  thread=$(uuid)
  echo
  echo "== $label: local/devbox's own devcontainer is the run's environment (thread $thread)"
  reset_journals "$label"
  preload
  own_base "$repo_path" "$thread"
  # The run's label lists a container while the run lasts: look every second until the task is over.
  ( while :; do podman_ps; sleep 1; done > "$tmp/containers-seen.txt" ) &
  watcher=$!
  message "In $repo_base/$repo_path.git (base branch main), record where devbox-tool runs in tool.txt. [mock:devcontainer]" "$tmp/run.json"
  stream "$tmp/run.json" "$label"
  kill "$watcher" 2>/dev/null || true
  wait "$watcher" 2>/dev/null || true
  watcher=

  check_published "$label" "$repo_path"
  step="Building the environment from .devcontainer/devcontainer.json ($repo_path)"
  step_ends "$label" "$step" completed
  export_thread
  if [ -f "$tmp/export.json" ]; then
    _logged=$(jq -r --arg p "$step" '[.events[] | select(.kind == "agent_step" and ((.data.label // "") | startswith($p)) and .data.phase == "end")] | last | .data.state // empty' "$tmp/export.json" 2>/dev/null || true)
    if [ "$_logged" = completed ]; then ok "$label: the thread's log has the step '$step' ending completed (an agent_step event)"; else bad "$label: the log's end of the step '$step' is '${_logged:-absent}', want completed"; fi
  fi
  # run_command, run_checks and OpenCode ran in the devcontainer: devbox-tool is only there.
  result_says "$label" dc-call-2 'devbox-tool 1.0 (from the devcontainer)' "run_command found devbox-tool, which only the devcontainer has"
  result_says "$label" dc-call-3 'PATH=' "the result of \`env\` in the devcontainer is there"
  # No secret of the coder is in the environment of a devcontainer.
  for secret in GITHUB_TOKEN DATABASE_URL A2A_BEARER_TOKENS MODEL_API_KEY dev-github-token dev-coder-token mock-api-key postgres://; do
    result_lacks "$label" dc-call-3 "$secret"
  done
  _kind=$(artifact_json checks .environment.kind)
  _source=$(artifact_json checks .environment.source)
  if [ "$_kind" = devcontainer ] && [ "$_source" = .devcontainer/devcontainer.json ]; then
    ok "$label: the checks artifact says the checks ran in a devcontainer, built from .devcontainer/devcontainer.json"
  else
    bad "$label: the checks artifact's environment is kind '${_kind:-absent}', source '${_source:-absent}', want devcontainer from .devcontainer/devcontainer.json"
  fi
  _oc=$(step_field OpenCode state)
  if [ "$_oc" = completed ]; then ok "$label: OpenCode was started (a sub-agent step) and ended completed"; else bad "$label: the OpenCode step is '${_oc:-absent}', want completed"; fi
  if [ "$cloned" = 1 ]; then
    _content=$(cat "$tmp/clone/tool.txt" 2>/dev/null || echo '<missing>')
    if [ "$_content" = 'devbox-tool 1.0 (from the devcontainer)' ]; then ok "$label: tool.txt on the branch says it came from the devcontainer (OpenCode's own bash command ran there)"; else bad "$label: tool.txt on the branch is '$_content', want 'devbox-tool 1.0 (from the devcontainer)'"; fi
  fi
  # The environment's life: a container while the run lasted, none after it.
  if [ -s "$tmp/containers-seen.txt" ]; then ok "$label: Podman listed a container with the run's label while the run lasted"; else bad "$label: Podman never listed a container with the run's label while the run lasted"; fi
  teardown "$label"
}

# --- default-env --------------------------------------------------------------------------------------------------
scenario_default_env() {
  label=default-env
  repo_path=local/sandbox
  thread=$(uuid)
  echo
  echo "== $label: local/sandbox has no devcontainer, so the run works in the default image (thread $thread)"
  reset_journals "$label"
  preload
  own_base "$repo_path" "$thread"
  message "In $repo_base/$repo_path.git (base branch main), add hello.txt containing hello. [mock:default-env]" "$tmp/run.json"
  stream "$tmp/run.json" "$label"

  check_published "$label" "$repo_path"
  step_ends "$label" "Using the default environment (" completed
  # Only the coder's own image has /opt/flutter: the output (not the command's own text) says where it ran.
  result_says "$label" de-call-2 "$(printf -- '--- output ---\ndevcontainer-env')" "the probe printed devcontainer-env, so it ran in the default environment, not in the coder's"
  _kind=$(artifact_json checks .environment.kind)
  _source=$(artifact_json checks .environment.source)
  if [ "$_kind" = devcontainer ] && [ -z "$_source" ]; then ok "$label: the checks artifact says the checks ran in a devcontainer made from the default image (no source file)"; else bad "$label: the checks artifact's environment is kind '${_kind:-absent}', source '${_source:-none}', want devcontainer with no source"; fi
  if [ "$cloned" = 1 ]; then
    _content=$(cat "$tmp/clone/hello.txt" 2>/dev/null || echo '<missing>')
    if [ "$_content" = hello ]; then ok "$label: hello.txt on the branch is 'hello'"; else bad "$label: hello.txt on the branch is '$_content', want 'hello'"; fi
  fi
  teardown "$label"
}

# --- broken-env ---------------------------------------------------------------------------------------------------
scenario_broken_env() {
  label=broken-env
  repo_path=local/devbox-broken
  thread=$(uuid)
  echo
  echo "== $label: local/devbox-broken's devcontainer.json cannot be used, and the person decides (thread $thread)"
  reset_journals "$label"
  message "In $repo_base/$repo_path.git (base branch main), record where devbox-tool runs in tool.txt. [mock:broken-env]" "$tmp/run.json"
  stream "$tmp/run.json" "$label"

  if [ "$outcome" = interrupt ] && [ "$state" = blocked ]; then
    ok "$label: the run ended RUN_FINISHED (interrupt) and the thread is blocked on the coder's question"
  else
    bad "$label: the run ended '${outcome:-none}' and the thread is '${state:-unknown}', want interrupt and blocked"
    why "$events"
    dump_logs
  fi
  no_pull_request "$label" "$repo_path"
  step="Building the environment from .devcontainer/devcontainer.json ($repo_path)"
  step_ends "$label" "$step" failed
  case $(step_field "$step" detail) in
    *privileged*) ok "$label: the failed step names privileged" ;;
    *) bad "$label: the failed step's detail does not name privileged: '$(step_field "$step" detail | head -c 300)'" ;;
  esac
  # What the model was told: the file, the key, and that the way on is the person's.
  for want in '.devcontainer/devcontainer.json' privileged ask_user rebuild_environment; do
    result_says "$label" be-call-2 "$want" "the result of the first command names '$want'"
  done
  case $said in *"go on in the default environment"*) ok "$label: the coder asks the person how to go on" ;; *) bad "$label: the coder's question about how to go on is not in what it said" ;; esac
  # The file's hostile parts did nothing. initializeCommand runs on the host side, which is the coder: it was removed.
  if dc exec -T coder test ! -e /work/INIT-RAN; then ok "$label: /work/INIT-RAN does not exist: the initializeCommand did not run"; else bad "$label: /work/INIT-RAN exists: the initializeCommand ran in the coder"; fi
  # The refused file made no container; and the coder's GITHUB_TOKEN, which `${localEnv:GITHUB_TOKEN}` asks for, is in none.
  _left=$(podman_ps)
  if [ -z "$_left" ]; then ok "$label: the refused file made no container with the run's label"; else bad "$label: Podman lists a container with the run's label: $_left"; fi
  _leaked=0
  for _c in $(podman_cli ps -a -q 2>/dev/null || true); do
    if podman_cli inspect --format '{{.Config.Env}}' "$_c" 2>/dev/null | grep -q 'dev-github-token'; then _leaked=$((_leaked + 1)); fi
  done
  if [ "$_leaked" = 0 ]; then ok "$label: the coder's GITHUB_TOKEN is in the environment of no container of the service (LEAK resolved to nothing, or never existed)"; else bad "$label: $_leaked container(s) of the service hold the coder's GITHUB_TOKEN"; fi
  export_thread
  if [ -f "$tmp/export.json" ]; then
    if grep -q 'dev-github-token' "$tmp/export.json"; then bad "$label: the thread's export holds the coder's GITHUB_TOKEN"; else ok "$label: the coder's GITHUB_TOKEN is not in the thread's export"; fi
  fi
}

# --- no-runtime ---------------------------------------------------------------------------------------------------
scenario_no_runtime() {
  label=no-runtime
  repo_path=local/devbox
  thread=$(uuid)
  echo
  echo "== $label: the Podman service is stopped, the run goes on in the coder's own container, and says so (thread $thread)"
  reset_journals "$label"
  if dc stop podman >/dev/null 2>&1; then podman_stopped=1; ok "$label: the Podman service is stopped"; else bad "$label: cannot stop the Podman service"; fi
  # The coder keeps a probe's answer for 30 seconds: a run that starts before that would still find the service.
  sleep 32
  message "In $repo_base/$repo_path.git (base branch main), record where devbox-tool runs in tool.txt. [mock:no-runtime]" "$tmp/run.json"
  stream "$tmp/run.json" "$label"
  if dc up -d --wait podman >/dev/null 2>&1; then podman_stopped=0; ok "$label: the Podman service is started again"; else bad "$label: cannot start the Podman service again"; fi

  if [ "$outcome" = interrupt ] && [ "$state" = blocked ]; then
    ok "$label: the run ended RUN_FINISHED (interrupt) and the thread is blocked on the coder's question"
  else
    bad "$label: the run ended '${outcome:-none}' and the thread is '${state:-unknown}', want interrupt and blocked"
    why "$events"
    dump_logs
  fi
  no_pull_request "$label" "$repo_path"
  step_ends "$label" "The container runtime is not reachable: commands run in the coder's own environment" completed
  # shellcheck disable=SC2016 # the backticks are the coder's own wording, not a command substitution
  result_says "$label" nr-call-2 'no `devbox-tool`' "devbox-tool is reported as a missing tool"
  case $said in *"commands run here without a container runtime"*) ok "$label: the coder asks the person what to do" ;; *) bad "$label: the coder's question is not in what it said" ;; esac
}

# --- run ----------------------------------------------------------------------------------------------------------
require_stack
privileges
for s in $scenarios; do
  case $s in
    devcontainer) scenario_devcontainer ;;
    default-env) scenario_default_env ;;
    broken-env) scenario_broken_env ;;
    no-runtime) scenario_no_runtime ;;
  esac
done

echo
finish
