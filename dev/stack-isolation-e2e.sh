#!/usr/bin/env sh
# Volumes are namespaced by compose project: `docker compose down -v` in one project leaves another project's volume
# and its data, and a project name shared by two checkouts makes one `down -v` destroy both. The rule that a checkout
# gets its own name from its directory is `tools/check-compose-project.sh`; this proves what the name buys.
#
#   dev/stack-isolation-e2e.sh
#
# It starts only `postgres`, in throwaway copies of compose.yaml (each a "checkout", with the init scripts it mounts),
# writes a table in each, and removes every project, volume and directory it makes — on failure too. Ports are picked
# free, so it runs beside a stack on the defaults and beside `dev/check-ports.sh`.
#
# Needs Docker with Compose v2. It prints one ok or FAIL line per check and exits 1 if any failed.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
pid=$$
work=$(mktemp -d)
fail=0

ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=$((fail + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi; }

bound() {
  if command -v ss >/dev/null 2>&1; then
    ss -ltn 2>/dev/null | awk 'NR > 1 {print $4}' | grep -qE "^(127\.0\.0\.1|\[::1\]|0\.0\.0\.0|\*|\[::\]):$1$"
  elif command -v netstat >/dev/null 2>&1; then
    netstat -ltn 2>/dev/null | awk 'NR > 1 {print $4}' | grep -qE "[:.]$1$"
  else
    lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
  fi
}

next_port() { # next_port BASE: the first free host port at or above BASE (a subshell, so no state between calls)
  _p=$1
  while bound "$_p"; do _p=$((_p + 1)); done
  printf '%s' "$_p"
}

checkout() { # checkout NAME: a directory holding compose.yaml and the files it mounts, as a clone would
  d="$work/$1"
  mkdir -p "$d/dev"
  cp "$root/compose.yaml" "$d/"
  cp -R "$root/dev/postgres-init" "$d/dev/"
  printf '%s' "$d"
}

cleanup() {
  for p in "$@"; do docker compose -p "$p" down -v --remove-orphans >/dev/null 2>&1 || true; done
  rm -rf "$work"
}
a="anr136-a-$pid"
b="anr136-b-$pid"
c="anr136-c-$pid"
trap 'cleanup "$a" "$b" "$c"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

up() { # up DIR PROJECT PORT
  ( cd "$1" && POSTGRES_PORT=$3 docker compose -p "$2" up -d --wait postgres >/dev/null 2>&1 )
}
down() { # down DIR PROJECT
  ( cd "$1" && docker compose -p "$2" down -v --remove-orphans >/dev/null 2>&1 )
}
psql() { # psql PROJECT SQL: the answer, trimmed
  docker compose -p "$1" exec -T postgres psql -U postgres -d orch -tAc "$2" 2>/dev/null | tr -d '[:space:]'
}
has_table() { # has_table PROJECT: 1 when the table this test writes is there
  psql "$1" "select count(*) from information_schema.tables where table_name = 'iso'"
}

echo "== two checkouts, two projects, one down -v"
da=$(checkout "$a")
db=$(checkout "$b")
base=$((15432 + (pid % 200)))
pa=$(next_port "$base")
pb=$(next_port $((pa + 1)))
if ! up "$da" "$a" "$pa" || ! up "$db" "$b" "$pb"; then
  bad "postgres did not start (ports $pa and $pb)"
else
  psql "$a" 'create table iso(x int); insert into iso values (1)' >/dev/null
  psql "$b" 'create table iso(x int); insert into iso values (2)' >/dev/null
  expect "each project has its own row" "$(psql "$a" 'select count(*) from iso')$(psql "$b" 'select count(*) from iso')" "11"
  expect "each project has a volume of its own" \
    "$(docker volume ls -q --filter "name=^${a}_postgres-data$" | wc -l | tr -d ' ')$(docker volume ls -q --filter "name=^${b}_postgres-data$" | wc -l | tr -d ' ')" "11"

  down "$da" "$a"
  expect "the torn-down project's volume is gone" "$(docker volume ls -q --filter "name=^${a}_postgres-data$" | wc -l | tr -d ' ')" "0"
  expect "the other project's volume remains" "$(docker volume ls -q --filter "name=^${b}_postgres-data$" | wc -l | tr -d ' ')" "1"
  expect "and its data is still there" "$(psql "$b" 'select count(*) from iso')" "1"
fi

echo "== two checkouts sharing one project name"
dc=$(checkout "$c")
dd=$(checkout "$c-b")
pc=$(next_port $((pb + 1)))
if ! up "$dc" "$c" "$pc" || ! up "$dd" "$c" "$pc"; then
  bad "postgres did not start under the shared name (port $pc)"
else
  psql "$c" 'create table iso(x int); insert into iso values (3)' >/dev/null
  expect "the first checkout has its table" "$(has_table "$c")" "1"
  # the second checkout's `down -v` names the same project: it removes the first one's volume too
  down "$dd" "$c"
  up "$dc" "$c" "$pc" || true
  expect "the second checkout's down -v took the first one's data with it" "$(has_table "$c")" "0"
fi

if [ "$fail" -eq 0 ]; then echo "stack isolation e2e passed"; else echo "stack isolation e2e FAILED"; exit 1; fi
