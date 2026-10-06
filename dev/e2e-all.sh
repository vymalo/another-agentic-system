#!/usr/bin/env sh
# Runs every scenario script of the local stack against a running `app` profile, one after the other, and
# prints a summary. This is the "does my stack work" command.
#
#   docker compose --profile app up -d --build --wait     # once (the first build takes several minutes)
#   dev/e2e-all.sh                                        # every scenario
#   dev/e2e-all.sh verify mcp                             # only these
#   VERBOSE=1 dev/e2e-all.sh                              # stream every script's own output
#
# The scenarios, each one script of this directory (the header of a script says exactly what it asserts):
#
#   greeting          "hi" gets a greeting that says the coder's name, not a task    greeting-e2e.sh
#   agents            three agents (coder, chat, researcher); each answers in its     agents-e2e.sh
#                     role on the mocks, the researcher with a source it searched for
#   choices           the coder asks three questions as one form drawn from the web's  choices-e2e.sh
#                     catalog, one action answers them, a newer catalog is recorded too
#   cards             the researcher searches and answers with one surface of cards    cards-e2e.sh
#                     and a graph under the web's catalog; an older screen keeps the
#                     thread's catalog; a screen without Cards gets words only
#   tools             a web search is attached to a chat (the run that creates the   tools-e2e.sh
#                     thread names it); the chat's model calls the relayed tool
#                     `websearch__web_search` and answers from its result; the call is
#                     ONE step with the icon `mcp-server:websearch`, its input and its
#                     output; the search is sent the configured bearer and header and
#                     no secret is in the export, the frames or the model's requests;
#                     after a detach the chat answers "No web search attached";
#                     GET /api/tool-servers lists the server with its icon; a plain
#                     agent's capabilities have no thread-tools key
#   steer             a message sent while an agent works (the chat on a model that   steer-e2e.sh
#                     calls a tool, then takes 20 s): Send is read by the running task, whose next model
#                     request ends with it, in one job with no second one after the
#                     turn; Stop & send ends the task `canceled` within 5 s, job 2
#                     starts and continues it, and the abandoned job is never judged
#   reasoning         a model that thinks before it answers (the chat on a model that  reasoning-e2e.sh
#                     writes `reasoning_content`, `[mock:think]`): the AG-UI stream has
#                     one reasoning span, in the order the protocol requires and before
#                     the reply, the log one `agent_reasoning` with the whole text and
#                     nowhere else, a reconnect says it once, and a second message's
#                     model request carries none of it (it is never sent back)
#   mentions          the owner's football sentence, mentioning @researcher @browser     mentions-e2e.sh
#                     @coder: a mention of nobody is 422 and writes nothing; the chat's
#                     model asks the three (WireMock agents) with `ask_agent`, one after
#                     the other: three `ask_started` by `main` in order, each
#                     `ask_finished` completed, each agent sent only its own request in
#                     its own context, the final message names all three answers, and
#                     the AG-UI stream has SUBAGENT_STARTED sub-ask-1..3 under the chat
#                     agent's run (the asked agents are mocks: no child steps, see the
#                     script's header)
#   title             a thread is titled by the orchestrator's model after the        title-e2e.sh
#                     agent's first reply; none or a failing model keeps the first
#                     words; a person's rename is final
#   description       a finished job gets the thread a description from its own       description-e2e.sh
#                     model at its own endpoint (guidance from the configuration, the
#                     core's form, data clause and language line around it); NONE or a
#                     failing model leaves none; a person's description (or clearing it)
#                     is final and a fork has it; GET /api/config
#   fork              a finished thread is forked through the API; the first message  fork-e2e.sh
#                     of the fork reaches the agent with the conversation it continues
#                     in front of it (read from the mock agent's request journal),
#                     the next one does not, and the parent's own message is plain
#   rail              the person's own list of threads (ADR 0042): pin, order, archive  rail-e2e.sh
#                     and eject over the API: the pinned come first in `order=rail`, a
#                     thread is put on top, before or after another, an archived one
#                     is left out and brought back where it was, a fork is nested under
#                     its parent and ejected on request; none of it is in the log, and
#                     another person's request is a 404
#   delete            deleting a thread erases it (ADR 0043): a thread that works is   delete-e2e.sh
#                     refused (409 thread_active), a cancelled one is deleted (204);
#                     another person's delete is a 404 that changes nothing; the owner's
#                     erases the thread with its files (the volume), its link (404 at
#                     once) and keeps a fork whole, with its own copy of the files; a
#                     second delete is 404; a model's late reply for a deleted thread
#                     is dropped and counted (needs docker: it reads the artifact volume)
#   registry          the platform's agent registry (mock-registry): its agent is     registry-e2e.sh
#                     listed after dev/agents.yaml's with the releases of its own card,
#                     an agent added to it shows up with no restart, a registry that
#                     is down leaves the static agents and says so (503 for its agents)
#   rbac              who may do what: the mock issuer signs four users in behind     rbac-e2e.sh
#                     oauth2-proxy, /api/me says what each one's roles grant, a user
#                     sees only their own threads, an administrator too: nobody lists
#                     (?owner= is 400) or reads another's thread (404, ADR 0039), a token for
#                     another audience is 401, `chat-only` is refused the coder (403)
#                     and is listed only the chat
#   coder             chat -> coder -> branch -> mock-ci -> green -> pull request   coder-e2e.sh
#                     (the work as a tree of steps, the answer shown as it is written)
#   coder-no-opencode the same, the check command makes the change (no OpenCode)    NO_OPENCODE=1 coder-e2e.sh
#   workspace         the coder needs no repository to start: a scratch project, a      workspace-e2e.sh
#                     repository it creates only after the person says yes (and none
#                     after no), a second repository that joins the workspace only
#                     after yes; each question is a form from the web's catalog, one
#                     action answers it; the gate, the CI card and the pull request
#                     are those of the repository the work reached
#   artifact          the coder makes three files (an SVG with a script in it, a PNG, a   artifact-e2e.sh
#                     JSON report) and shares them with `share_file`; the orchestrator
#                     keeps them in its artifact store and the log holds only the
#                     references (`vymalo.artifact`, href and sha256); the surface that
#                     places two of them as `Image`s comes after the files; the API
#                     serves the bytes (right type, nosniff, sandbox CSP, the SVG
#                     sanitized, a download the original) to the owner and 404s
#                     another user, another thread and a wrong hash
#   verify            red once -> rework -> green; red always -> failed; the gate    verify-e2e.sh
#                     cannot be weakened by a run
#   verifier          the verifier finds fault -> rework -> the verifier passes      verifier-e2e.sh
#   mcp               an MCP client starts a job and follows it, with progress       mcp-e2e.sh
#   ci                a signed CI report sends the agent back, then ends the job     ci-e2e.sh
#   folder            the coder restarted on a copy of its agent folder with another  agent-folder-e2e.sh
#                     name says that name, then on its own folder says its own again
#
# `ci` passes once per database (a commit belongs to the first job that pushed it): on a second run it is
# SKIPPED, not failed, and the summary says how to reset (`docker compose --profile app down -v`).
# `folder` restarts the coder (it runs last, and puts the coder back on its folder when it ends) and needs
# `docker compose` on the machine that runs the stack: without it, it is SKIPPED too.
# The split roles (dev/split-e2e.sh) need another shape of the stack and are not part of this list; neither is dev/devcontainer-e2e.sh, which
# needs the stack WITH -f dev/compose.devcontainer.yaml (a rootless Podman service beside the coder: dev/README.md, "Devcontainers").
#
# Each script's output goes to a file, and only the tail of a failing one is printed; the file is kept in
# $LOG_DIR (default: a fresh directory under ${TMPDIR:-/tmp}) and named in the summary. Environment that the
# scripts read (BASE_URL, EDGE_PORT, AUTH_EMAIL, MOCK_OIDC_PORT, TIMEOUT, GITHUB_AUTH, ...) is passed through unchanged. Every script gets
# its identity from the mock issuer (dev/auth-header.sh). (GITHUB_AUTH=app
# says the stack runs the coder as a GitHub App, `-f dev/compose.github-app.yaml`; `folder` restarts the coder WITHOUT that override, so
# run `folder` on the default stack.)
#
# Exit status: 0 when no scenario failed (a skip is not a failure), 1 when one did, 2 on a usage error or
# when the stack is not there. Needs curl, jq, git and openssl (and docker compose, to print logs, and for `delete`, which reads the
# artifact volume and the metrics through it).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
export BASE_URL="$base"

all="greeting agents choices cards tools steer reasoning mentions title description fork rail delete registry rbac coder coder-no-opencode workspace artifact verify verifier mcp ci folder"
# shellcheck disable=SC2086 # the list is words on purpose
[ "$#" -gt 0 ] || set -- $all
for s in "$@"; do
  case " $all " in
    *" $s "*) ;;
    *) echo "unknown scenario '$s'; choose from: $all" >&2; exit 2 ;;
  esac
done

for tool in curl jq git openssl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required and was not found" >&2; exit 2; }
done

# --- is the stack there? ---------------------------------------------------------------------------------
if ! curl -fsS --max-time 10 "$base/readyz" >/dev/null 2>&1; then
  cat >&2 <<EOF
Nothing answers at $base/readyz. Start the stack first (the first build takes several minutes, and the
coder image is about 2.9 GB):

  docker compose --profile app up -d --build --wait

or point BASE_URL (or EDGE_PORT) at the edge you started. See dev/README.md, "Test it locally".
EOF
  exit 2
fi
if ! id_header=$(sh "$here/auth-header.sh" "${AUTH_EMAIL:-dev@example.com}"); then
  echo "no token from the mock issuer: is the stack up with the mock-oidc service (docker compose --profile app up -d --build --wait)? See dev/README.md, \"Sign in\"." >&2
  exit 2
fi
agents=$(curl -fsS --max-time 30 -H "$id_header" "$base/api/agents" 2>/dev/null | jq -r '[.[].id] | join(" ")' 2>/dev/null || true)
echo "stack: $base, agents: ${agents:-none}"
for s in "$@"; do
  case $s in
    greeting | choices | coder | coder-no-opencode | workspace | folder)
      case " $agents " in
        *" adam "*) ;;
        *) echo "scenario $s needs the agent 'adam', which GET /api/agents does not list: is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
      esac ;;
    rbac)
      for a in adam chat researcher; do
        case " $agents " in
          *" $a "*) ;;
          *) echo "scenario rbac needs the agents adam, chat and researcher; GET /api/agents does not list '$a' (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml, and the roles of dev/orchestrator.yaml?" >&2; exit 2 ;;
        esac
      done ;;
    fork | rail)
      case " $agents " in
        *" mock-coder "*) ;;
        *) echo "scenario $s needs the agent 'mock-coder', which GET /api/agents does not list (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
      esac ;;
    artifact | delete)
      for a in coder-share chat; do
        case " $agents " in
          *" $a "*) ;;
          *) echo "scenario $s needs the agents coder-share and chat; GET /api/agents does not list '$a' (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
        esac
      done ;;
    steer)
      case " $agents " in
        *" chat "*) ;;
        *) echo "scenario steer needs the agent 'chat', which GET /api/agents does not list (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
      esac ;;
    reasoning)
      case " $agents " in
        *" chat "*) ;;
        *) echo "scenario reasoning needs the agent 'chat', which GET /api/agents does not list (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
      esac ;;
    mentions)
      for a in chat mock-researcher mock-browser mock-coder; do
        case " $agents " in
          *" $a "*) ;;
          *) echo "scenario mentions needs the agents chat, mock-researcher, mock-browser and mock-coder; GET /api/agents does not list '$a' (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
        esac
      done ;;
    tools)
      for a in chat mock-coder; do
        case " $agents " in
          *" $a "*) ;;
          *) echo "scenario tools needs the agents chat and mock-coder; GET /api/agents does not list '$a' (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
        esac
      done ;;
    cards)
      case " $agents " in
        *" researcher "*) ;;
        *) echo "scenario cards needs the agent 'researcher', which GET /api/agents does not list (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
      esac ;;
    agents)
      for a in adam chat researcher; do
        case " $agents " in
          *" $a "*) ;;
          *) echo "scenario agents needs the agents adam, chat and researcher; GET /api/agents does not list '$a' (it lists: ${agents:-none}): is this the app profile of compose.yaml, with dev/agents.yaml?" >&2; exit 2 ;;
        esac
      done ;;
  esac
done

log_dir=${LOG_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/e2e-all.XXXXXX")}
mkdir -p "$log_dir"
summary=$(mktemp)
trap 'rm -f "$summary"' EXIT

passed=0
failed=0
skipped=0

# run NAME COMMAND...: runs one scenario, records its result in $summary.
run() {
  name=$1
  shift
  log=$log_dir/$name.log
  printf '== %s\n' "$name"
  started=$(date +%s)
  if [ "${VERBOSE:-}" = 1 ]; then
    # `tee` would hide the script's status, so the status goes through a file.
    { rc=0; "$@" 2>&1 || rc=$?; echo "$rc" >"$log.rc"; } | tee "$log"
    rc=$(cat "$log.rc")
  else
    rc=0
    "$@" >"$log" 2>&1 || rc=$?
  fi
  took=$(($(date +%s) - started))
  case $rc in
    0)
      passed=$((passed + 1))
      printf 'ok    %-18s %4ss\n' "$name" "$took" | tee -a "$summary" ;;
    77)
      skipped=$((skipped + 1))
      printf 'SKIP  %-18s %4ss  (%s)\n' "$name" "$took" "$(sed -n 's/^SKIP  *//p' "$log" | head -n 1 | cut -c1-100)" | tee -a "$summary"
      sed -n 's/^      //p' "$log" | head -n 2 | sed 's/^/      /' ;;
    *)
      failed=$((failed + 1))
      printf 'FAIL  %-18s %4ss  log: %s\n' "$name" "$took" "$log" | tee -a "$summary"
      if [ "${VERBOSE:-}" != 1 ]; then
        echo "  --- the last lines of its output"
        tail -n 25 "$log" | sed 's/^/  | /'
      fi ;;
  esac
}

for s in "$@"; do
  case $s in
    greeting) run greeting sh "$here/greeting-e2e.sh" ;;
    agents) run agents sh "$here/agents-e2e.sh" ;;
    choices) run choices sh "$here/choices-e2e.sh" ;;
    cards) run cards sh "$here/cards-e2e.sh" ;;
    tools) run tools sh "$here/tools-e2e.sh" ;;
    steer) run steer sh "$here/steer-e2e.sh" ;;
    reasoning) run reasoning sh "$here/reasoning-e2e.sh" ;;
    mentions) run mentions sh "$here/mentions-e2e.sh" ;;
    title) run title sh "$here/title-e2e.sh" ;;
    description) run description sh "$here/description-e2e.sh" ;;
    fork) run fork sh "$here/fork-e2e.sh" ;;
    rail) run rail sh "$here/rail-e2e.sh" ;;
    delete) run delete sh "$here/delete-e2e.sh" ;;
    registry) run registry sh "$here/registry-e2e.sh" ;;
    rbac) run rbac sh "$here/rbac-e2e.sh" ;;
    coder) run coder sh "$here/coder-e2e.sh" ;;
    coder-no-opencode) run coder-no-opencode env NO_OPENCODE=1 sh "$here/coder-e2e.sh" ;;
    workspace) run workspace sh "$here/workspace-e2e.sh" ;;
    artifact) run artifact sh "$here/artifact-e2e.sh" ;;
    verify) run verify sh "$here/verify-e2e.sh" ;;
    verifier) run verifier sh "$here/verifier-e2e.sh" ;;
    mcp) run mcp sh "$here/mcp-e2e.sh" ;;
    ci) run ci sh "$here/ci-e2e.sh" ;;
    folder) run folder sh "$here/agent-folder-e2e.sh" ;;
  esac
done

echo
echo "== summary ($base)"
cat "$summary"
echo "$passed passed, $failed failed, $skipped skipped; logs in $log_dir"
if grep -q '^SKIP  ci ' "$summary"; then
  echo "ci passed in an earlier run of this database; to run it again from scratch:"
  echo "  docker compose --profile app down -v && docker compose --profile app up -d --build --wait"
fi
if [ "$failed" -gt 0 ]; then
  echo "the logs of the stack: docker compose --profile app logs --no-color --tail 100 orchestrator mock-ci adam chat researcher"
  exit 1
fi
exit 0
