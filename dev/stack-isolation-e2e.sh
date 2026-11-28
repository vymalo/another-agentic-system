#!/usr/bin/env sh
# Two stacks side by side: `docker compose down -v` in one leaves the other's volume, and a shared project name
# destroys both. It is the check behind "One project per checkout" (dev/README.md): compose.yaml pins no `name:`, so
# Compose derives the project from the directory, and containers, networks and volumes carry its prefix.
#
#   dev/stack-isolation-e2e.sh
#
# It starts only `postgres` (the image is already local after any `up`), writes a table in each project, and tears
# down what it made. Nothing else is touched: every project it creates is named anr136-a-<pid> / anr136-b-<pid> /
# anr136-s-<pid>, and all three are removed at the end, on failure too.
#
# Needs Docker with Compose v2. It prints one ok or FAIL line per check and exits 1 if any failed.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
pid=$$
a="anr136-a-$pid"
b="anr136-b-$pid"
s="anr136-s-$pid"
fail=0

ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=$((fail + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi; }

cleanup() {
  for p in "$a" "$b" "$s"; do docker compose -p "$p" down -v --remove-orphans >/dev/null 2>&1 || true; done
}
trap cleanup EXIT INT TERM

psql() { # psql PROJECT SQL: the answer, one line
  docker compose -p "$1" exec -T postgres psql -U postgres -d orch -tAc "$2" 2>/dev/null | tr -d '[:space:]'
}

up() { # up PROJECT PORT: postgres alone, on a host port of its own (host ports are not per project)
  POSTGRES_PORT=$2 docker compose -p "$1" up -d --wait postgres >/dev/null 2>&1
}

echo "== two projects, one checkout, one down -v"
up "$a" 15432 || { echo "postgres did not start for $a" >&2; exit 1; }
up "$b" 15433 || { echo "postgres did not start for $b" >&2; exit 1; }
psql "$a" 'create table iso(x int); insert into iso values (1)' >/dev/null
psql "$b" 'create table iso(x int); insert into iso values (2)' >/dev/null
expect "each project has its own row" "$(psql "$a" 'select count(*) from iso')$(psql "$b" 'select count(*) from iso')" "11"

volumes_a=$(docker volume ls -q --filter "name=^${a}_postgres-data$" | wc -l | tr -d ' ')
volumes_b=$(docker volume ls -q --filter "name=^${b}_postgres-data$" | wc -l | tr -d ' ')
expect "each project has a volume of its own" "$volumes_a$volumes_b" "11"

docker compose -p "$a" down -v >/dev/null 2>&1
expect "the torn-down project's volume is gone" "$(docker volume ls -q --filter "name=^${a}_postgres-data$" | wc -l | tr -d ' ')" "0"
expect "the other project's volume remains" "$(docker volume ls -q --filter "name=^${b}_postgres-data$" | wc -l | tr -d ' ')" "1"
expect "and its data is still there" "$(psql "$b" 'select count(*) from iso')" "1"

echo "== one shared project name (the failure this rule prevents)"
up "$s" 15434
psql "$s" 'create table iso(x int); insert into iso values (3)' >/dev/null
docker compose -p "$s" down -v >/dev/null 2>&1
up "$s" 15434
expect "with a shared name, down -v in one takes the other's data with it" "$(psql "$s" "select count(*) from information_schema.tables where table_name = 'iso'")" "0"

if [ "$fail" -eq 0 ]; then echo "stack isolation e2e passed"; else echo "stack isolation e2e FAILED"; exit 1; fi
