#!/usr/bin/env sh
# System-level test of an agent configured at run time (MVP slice 1): a change to the folder the coder
# reads changes its answer after a restart, with no rebuild.
#
#   dev/agent-folder-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only):
#
#   docker compose --profile app up -d --build --wait
#
# The coder reads its agent folder (instructions, card) once, at startup, from the directory mounted at
# /etc/adam/agent (ADAM_AGENT_DIR in compose.yaml): dev/coder/agent, or the directory CODER_AGENT_DIR names.
# The script:
#   1. copies that folder to a temporary directory and gives the copy another name (`display_name: Cody` in
#      the instructions, `card.name: Cody`);
#   2. restarts the coder on the copy: `CODER_AGENT_DIR=<copy> docker compose --profile app up -d --no-build
#      --wait coder`. The same image, no build, only a new container;
#   3. checks the coder's own card (public, on CODER_PORT) names Cody, then runs dev/greeting-e2e.sh against the
#      copy: a new thread says "hi" and the answer is "I'm Cody", and the system prompt the model got says
#      "Your name is Cody.";
#   4. puts the coder back on the folder it was on (also when a check failed, or on Ctrl-C: a trap) and runs
#      dev/greeting-e2e.sh against that folder again: "I'm Coder".
# A greeting right after a restart can meet a connection the orchestrator still holds to the old container; each
# greeting is tried up to three times, five seconds apart, and fails only when all three fail.
#
# It prints one ok or FAIL line per step and exits 1 if any failed. It exits 77 (a skip, see dev/e2e-all.sh) when
# `docker compose` is not here or the coder is not a service of this compose project: it needs the Docker
# daemon of the machine that runs the stack, because the copy is a directory of this machine.
#
# Environment:
#   CODER_AGENT_DIR  unset     the folder the stack mounts (the default dev/coder/agent when unset); read here
#                              as the folder to copy, and put back at the end
#   CODER_PORT       8090      where the coder's card is served on this machine
#   NEW_NAME         Cody      the name of the copy
#   the variables of dev/greeting-e2e.sh (BASE_URL, EDGE_PORT, AUTH_EMAIL, MOCK_OPENAI_PORT, ...), passed through
#
# Needs curl, jq, docker compose. Verified by CI only, in .github/workflows/coder-e2e.yml. It is for the offline
# stack (compose.yaml): restarting the coder of `-f compose.live.yaml` with this command would drop the override.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
original=${CODER_AGENT_DIR:-$root/dev/coder/agent}
coder_url=http://127.0.0.1:${CODER_PORT:-8090}
new_name=${NEW_NAME:-Cody}

compose() { docker compose -f "$root/compose.yaml" --profile app "$@"; }
# compose_on FOLDER ARGS...: the same with the folder mounted at /etc/adam/agent (an `env` prefix, not a shell
# assignment: before a function call, a shell may keep the assignment after the call returns).
compose_on() {
  folder=$1
  shift
  env CODER_AGENT_DIR="$folder" docker compose -f "$root/compose.yaml" --profile app "$@"
}

if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
  echo "SKIP  docker compose is not available here: this scenario restarts the coder on another folder." >&2
  exit 77
fi
if [ -z "$(compose ps -q coder 2>/dev/null || true)" ]; then
  echo "SKIP  the coder is not a service of this compose project (start it: docker compose --profile app up -d --build --wait)." >&2
  exit 77
fi
if [ ! -f "$original/instructions.md" ]; then
  echo "FAIL $original/instructions.md does not exist (CODER_AGENT_DIR is the folder that holds instructions.md)"
  exit 1
fi

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
finish() {
  if [ "$fail" -eq 0 ]; then echo "agent folder e2e passed"; else echo "agent folder e2e FAILED"; exit 1; fi
}

tmp=$(mktemp -d)
restart_needed=0
# Put the coder back on the folder it was on, whatever happens. The environment of this script is the caller's, so
# an unset CODER_AGENT_DIR gives the default mount again.
restore() {
  status=$?
  trap - EXIT INT TERM
  if [ "$restart_needed" = 1 ]; then
    restart_needed=0
    compose up -d --no-build --wait --wait-timeout 180 coder >/dev/null 2>&1 ||
      echo "could not put the coder back on its folder: run: docker compose --profile app up -d coder"
  fi
  rm -rf "$tmp"
  exit "$status"
}
trap restore EXIT
trap 'exit 130' INT TERM

# greet FOLDER: dev/greeting-e2e.sh against the folder the coder runs on, up to three tries.
greet() {
  try=1
  while :; do
    if AGENT_DIR=$1 sh "$root/dev/greeting-e2e.sh" >"$tmp/greeting.out" 2>&1; then
      sed 's/^/     /' "$tmp/greeting.out"
      return 0
    fi
    sed 's/^/     /' "$tmp/greeting.out"
    if [ "$try" -ge 3 ]; then return 1; fi
    echo "     try $try failed; again in 5 s"
    try=$((try + 1))
    sleep 5
  done
}

card_name() { curl -fsS --max-time 30 "$coder_url/.well-known/agent-card.json" 2>/dev/null | jq -r '.name // empty' 2>/dev/null || true; }

# --- 0. the coder as it is ----------------------------------------------------------------------------
old_name=$(sed -n 's/^[[:space:]]*display_name:[[:space:]]*//p' "$original/instructions.md" | head -n 1)
echo "the coder runs on $original ($old_name); a copy will be called $new_name"
if [ "$old_name" = "$new_name" ]; then
  bad "the folder is already called $new_name: set NEW_NAME to another name"
  finish
fi
before=$(card_name)
if [ -n "$before" ]; then ok "the coder's card says: $before"; else bad "cannot read the coder's card at $coder_url/.well-known/agent-card.json"; fi

# --- 1. a copy with another name -----------------------------------------------------------------------
copy=$tmp/agent
mkdir "$copy"
cp -R "$original"/. "$copy"/
# `display_name` (the var the body renders) and `card.name`, which the folder keeps in step (the indented `name:`).
sed -e "s/^\([[:space:]]*display_name:\)[[:space:]]*.*/\1 $new_name/" \
  -e "s/^\([[:space:]][[:space:]]*name:\)[[:space:]]*${old_name}[[:space:]]*\$/\1 $new_name/" \
  "$original/instructions.md" >"$copy/instructions.md"
# The container runs as uid 10001: the copy must be readable by others (mktemp makes the directory 0700).
chmod -R a+rX "$tmp"
if grep -q "^[[:space:]]*display_name:[[:space:]]*$new_name\$" "$copy/instructions.md"; then
  ok "the copy $copy is called $new_name"
else
  bad "the copy has no 'display_name: $new_name' line"
  finish
fi

# --- 2. restart the coder on it: no build ---------------------------------------------------------------
restart_needed=1
if compose_on "$copy" up -d --no-build --wait --wait-timeout 180 coder >"$tmp/up.out" 2>&1; then
  ok "the coder restarted on the copy (up -d --no-build: the same image)"
else
  bad "the coder did not come back with CODER_AGENT_DIR=$copy: $(tail -n 5 "$tmp/up.out")"
  finish
fi

# --- 3. the new name ------------------------------------------------------------------------------------
after=$(card_name)
if [ "$after" = "$new_name" ]; then ok "the coder's card now says: $after"; else bad "the coder's card says '${after:-nothing}', want $new_name"; fi
if greet "$copy"; then ok "a new thread: \"hi\" gets \"I'm $new_name\" (dev/greeting-e2e.sh)"; else bad "the greeting after the restart on the copy failed (output above)"; fi

# --- 4. put it back --------------------------------------------------------------------------------------
restart_needed=0
if compose up -d --no-build --wait --wait-timeout 180 coder >"$tmp/up.out" 2>&1; then
  ok "the coder restarted on its own folder again"
  again=$(card_name)
  if [ "$again" = "$before" ]; then ok "the coder's card says: $again"; else bad "the coder's card says '${again:-nothing}', want ${before:-the first name}"; fi
  if greet "$original"; then ok "a new thread: \"hi\" gets \"I'm $old_name\" again"; else bad "the greeting after the restart on its own folder failed (output above)"; fi
else
  restart_needed=1
  bad "the coder did not come back on its own folder: $(tail -n 5 "$tmp/up.out")"
fi

finish
