#!/usr/bin/env sh
# Refuses to start a stack whose host ports are taken, naming the variable that moves each one.
#
# Every published port of compose.yaml is `127.0.0.1:${VAR:-default}:containerPort`, so a stack on a busy
# default fails inside Docker with "port is already allocated" and no word about VAR. This asks first:
#
#   dev/check-ports.sh                 # every defaulted port of compose.yaml
#   dev/check-ports.sh compose.live.yaml
#
# It reads the variables it is given, like Compose does (POSTGRES_PORT=15432 dev/check-ports.sh passes when
# 15432 is free). Exit 0 when every port it will publish is free, 1 with one line per port that is not, 2 when
# it cannot tell (no ss, netstat or lsof).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
files=${*:-"$root/compose.yaml"}

bound() { # bound PORT: true when something listens on 127.0.0.1:PORT
  if command -v ss >/dev/null 2>&1; then
    ss -ltn 2>/dev/null | awk '{print $4}' | grep -qE "^(127\.0\.0\.1|\[::1\]|0\.0\.0\.0|\*|\[::\]):$1$"
  elif command -v netstat >/dev/null 2>&1; then
    netstat -ltn 2>/dev/null | awk '{print $4}' | grep -qE "[:.]$1$"
  elif command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
  else
    return 2
  fi
}

free_port() { # free_port PORT: the next port above it that is free, for the suggestion
  _p=$1
  _n=0
  while [ "$_n" -lt 200 ]; do
    _p=$((_p + 1))
    bound "$_p" || { printf '%s' "$_p"; return 0; }
    _n=$((_n + 1))
  done
  printf '%s' ""
}

taken=0
for file in $files; do
  [ -f "$file" ] || { echo "no such file: $file" >&2; exit 2; }
  # service, variable and default of every `127.0.0.1:${VAR:-default}:port` binding
  awk '
    /^  [A-Za-z0-9_.-]+:$/ { svc = $1; sub(":$", "", svc); next }
    match($0, /127\.0\.0\.1:\$\{[A-Za-z_][A-Za-z0-9_]*:-[0-9]+\}:[0-9]+/) {
      b = substr($0, RSTART, RLENGTH)
      sub(/^127\.0\.0\.1:\$\{/, "", b)   # POSTGRES_PORT:-5432}:5432
      split(b, parts, "}")
      split(parts[1], vd, ":-")            # POSTGRES_PORT, 5432
      print svc "\t" vd[1] "\t" vd[2]
    }
  ' "$file" | while IFS='	' read -r svc var def; do
    # what Compose will publish: the variable when it is set and not empty (`:-`), the default otherwise
    val=$(printenv "$var" 2>/dev/null || true)
    [ -n "$val" ] || val=$def
    bound "$val"
    case $? in
      0)
        alt=$(free_port "$val")
        if [ -n "$alt" ]; then
          printf 'port %s is in use (%s, %s): stop what holds it, or set %s=%s\n' "$val" "$var" "$svc" "$var" "$alt"
        else
          printf 'port %s is in use (%s, %s): stop what holds it, or set %s\n' "$val" "$var" "$svc" "$var"
        fi
        exit 1
        ;;
      2)
        echo "cannot tell whether port $val is free: no ss, netstat or lsof" >&2
        exit 2
        ;;
    esac
  done || taken=$?
  [ "$taken" -eq 0 ] || break
done

if [ "$taken" -ne 0 ]; then
  echo "one port above is taken; the stack would fail to start" >&2
  exit "$taken"
fi
echo "ok: every host port of $files is free"
