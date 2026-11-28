#!/usr/bin/env sh
# Refuses to start a stack whose host ports are taken, naming the variable that moves each one.
#
# Every published port of compose.yaml is `127.0.0.1:${VAR:-default}:containerPort`, so a stack on a busy
# default fails inside Docker with "port is already allocated" and no word about VAR. This asks first, for
# the profiles you are about to start:
#
#   dev/check-ports.sh                          # what `--profile app` publishes (the documented start)
#   dev/check-ports.sh --profile s3             # that profile too
#   eval "$(dev/check-ports.sh --suggest)"      # the VAR=free-port lines to paste (nothing when all are free)
#   POSTGRES_PORT=15432 dev/check-ports.sh      # what Compose would publish with that set
#   dev/check-ports.sh compose.yaml compose.live.yaml   # an override is read with its base
#
# It reads the environment first, then `.env` (Compose does the same), and a variable that is set and empty
# is the default, as `:-` says. Exit 0 when every port it would publish is free, 1 with one line per taken
# port, 2 when it cannot tell (no ss, netstat or lsof, or a binding it cannot read).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
profiles=""
suggest=0
files=""
while [ $# -gt 0 ]; do
  case $1 in
    --profile)
      [ $# -ge 2 ] || { echo "--profile needs a name" >&2; exit 2; }
      profiles="$profiles $2"
      shift
      ;;
    --profile=*) profiles="$profiles ${1#--profile=}" ;;
    --suggest) suggest=1 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) files="$files $1" ;;
  esac
  shift
done
[ -n "$profiles" ] || profiles=" app"

# A file argument is relative to the repository root, as the header's examples are.
resolved=""
for f in $files; do
  case $f in
    /*) resolved="$resolved $f" ;;
    *) resolved="$resolved $root/$f" ;;
  esac
done
[ -n "$resolved" ] || resolved="$root/compose.yaml"
for f in $resolved; do
  [ -f "$f" ] || { echo "no such file: $f" >&2; exit 2; }
done

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

bound() { # bound PORT: true when something listens on 127.0.0.1:PORT
  if command -v ss >/dev/null 2>&1; then
    ss -ltn 2>/dev/null | awk 'NR > 1 {print $4}' | grep -qE "^(127\.0\.0\.1|\[::1\]|0\.0\.0\.0|\*|\[::\]):$1$"
  elif command -v netstat >/dev/null 2>&1; then
    netstat -ltn 2>/dev/null | awk 'NR > 1 {print $4}' | grep -qE "[:.]$1$"
  elif command -v lsof >/dev/null 2>&1; then
    # lsof sees only the invoking user's sockets: a listener of another user reads as free here.
    lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
  else
    return 2
  fi
}

value_of() { # value_of VAR DEFAULT: what Compose will publish
  _v=$(printenv "$1" 2>/dev/null || true)
  if [ -z "$_v" ] && [ -f "$root/.env" ]; then
    _v=$(sed -n "s/^$1=//p" "$root/.env" | tail -1 | sed 's/^"\(.*\)"$/\1/')
  fi
  [ -n "$_v" ] || _v=$2
  printf '%s' "$_v"
}

# `+` a published port of an enabled service, `-` one of a profile nobody asked for, `!` a binding this cannot
# read (it fails the run rather than being skipped).
: >"$tmp"
for f in $resolved; do
  awk -v profiles="$profiles" '
    /^  [A-Za-z0-9_.-]+:$/ { svc = $1; sub(":$", "", svc); prof = ""; inprof = 0; inports = 0; next }
    /^    profiles:/ {
      v = $0; sub(/^    profiles:[ ]*/, "", v); gsub(/[][",]/, "", v)
      if (v != "") prof = v; else inprof = 1
      next
    }
    inprof && /^      - / { v = $0; sub(/^      - /, "", v); gsub(/[][",]/, "", v); prof = (prof == "" ? v : prof "," v); next }
    inprof { inprof = 0 }
    /^    ports:/ { inports = 1; next }
    inports && /^    [A-Za-z]/ { inports = 0 }
    inports && /- "/ {
      if (match($0, /127\.0\.0\.1:\$\{[A-Za-z_][A-Za-z0-9_]*:-[0-9]+\}:[0-9]+/)) {
        b = substr($0, RSTART, RLENGTH)
        sub(/^127\.0\.0\.1:\$\{/, "", b)
        split(b, parts, "}")
        split(parts[1], vd, ":-")
        cport = parts[2]; sub(/^:/, "", cport)
        on = (prof == "") ? 1 : 0
        n = split(prof, ps, ",")
        for (i = 1; i <= n; i++) if (index(profiles " ", " " ps[i] " ") > 0) on = 1
        printf "%s\t%s\t%s\t%s\t%s\n", (on ? "+" : "-"), svc, vd[1], vd[2], cport
      } else if ($0 ~ /127\.0\.0\.1:/) {
        printf "!\t%s\t%s\n", svc, $0
      }
    }
  ' "$f" >>"$tmp"
done

taken=0
unknown=0
all=""
while IFS='	' read -r flag a b c _; do
  case $flag in
    '!')
      echo "cannot read a binding of $a: $b" >&2
      unknown=1
      ;;
    +)
      val=$(value_of "$b" "$c")
      all="$all $val"
      if bound "$val"; then
        taken=1
        printf 'port %s is in use (%s, %s)\n' "$val" "$b" "$a"
      fi
      ;;
  esac
done <"$tmp"

if [ "$unknown" -ne 0 ]; then
  echo "a binding above is not in the shape this script reads; check it by hand" >&2
  exit 2
fi

if [ "$taken" -eq 0 ]; then
  if [ "$suggest" -eq 1 ]; then echo "nothing to move: every host port of the profiles${profiles} is free" >&2; else
    echo "ok: every host port of the profiles${profiles} is free"
  fi
  exit 0
fi

if [ "$suggest" -eq 0 ]; then
  echo "one or more ports above are taken; the stack would fail to start (dev/check-ports.sh --suggest prints replacements)" >&2
  exit 1
fi

# A replacement is free on the host AND not one of the stack's own ports: the successor of 8080 is 8081, which
# is the mock agents' default.
while IFS='	' read -r flag a b c _; do
  [ "$flag" = "+" ] || continue
  val=$(value_of "$b" "$c")
  bound "$val" || continue
  candidate=$((val + 1))
  n=0
  while [ "$n" -lt 1000 ] && [ "$candidate" -lt 65535 ]; do
    clash=0
    for used in $all; do
      if [ "$used" = "$candidate" ]; then clash=1; break; fi
    done
    if [ "$clash" -eq 0 ] && ! bound "$candidate"; then break; fi
    candidate=$((candidate + 1))
    n=$((n + 1))
  done
  printf '%s=%s\n' "$b" "$candidate"
done <"$tmp"
exit 0
