#!/usr/bin/env sh
# The embedded build stays a library, not a server (ADR 0076 decision 1: "a CI job fails when the
# embedded build's normal dependency tree holds sqlx, an HTTP server, ...").
#
# `orch-embed` is the library composition root an application links: it composes the app and the
# SQLite store and speaks no protocol. What must never appear on its **normal** edges is a web
# server, a Postgres driver or sqlx — a desktop or a phone has none of them, and one Cargo.toml
# edit away they would be pulled in silently. Dev-dependencies are out of scope: they are not
# linked by an application.
#
# Usage: tools/embedded-deps/check.sh [crate]           (default: orch-embed)
#        run from the repository root; the workspace is orchestrator/
# Exit 0 = the normal tree holds none of the four families and one libsqlite3-sys.
# Exit 1 = one is there (each is named), or the tree cannot be read.
set -eu

crate=${1:-orch-embed}
workspace=orchestrator

# The four families ADR 0076 names, matched as dash-separated words so `sqlx-core`,
# `tokio-postgres-rustls` and `hyper-util` are caught with their parent (the check exists so that a
# server cannot arrive under a new name).
banned='(^|-)(axum|sqlx|hyper|tokio-postgres)(-|$)'

if [ ! -d "$workspace" ]; then
  echo "::error::$workspace does not exist; run this from the repository root." >&2
  exit 1
fi

tree=$( cd "$workspace" && cargo tree -p "$crate" --edges normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u ) || {
  echo "::error::cargo tree could not read $crate's normal dependencies." >&2
  exit 1
}
if [ -z "$tree" ]; then
  echo "::error::cargo tree answered nothing for $crate." >&2
  exit 1
fi

found=$(printf '%s\n' "$tree" | grep -E "$banned" || true)
if [ -n "$found" ]; then
  echo "::error::$crate's normal dependency tree holds a server or a database driver (ADR 0076 decision 1):" >&2
  printf '%s\n' "$found" >&2
  echo "Find what brings it in with: (cd $workspace && cargo tree -p $crate -i <package> --edges normal)" >&2
  exit 1
fi

# One SQLite driver, at one version: two `libsqlite3-sys` are two native libraries linked into the
# application, which is not a thing a phone build survives.
sqlite=$(printf '%s\n' "$tree" | grep -x 'libsqlite3-sys' || true)
if [ -z "$sqlite" ]; then
  echo "::error::$crate's normal tree has no libsqlite3-sys, so it does not carry the SQLite store it is built on." >&2
  exit 1
fi
version=$( cd "$workspace" && cargo tree -p "$crate" --edges normal --prefix none -i libsqlite3-sys 2>/dev/null | sed -n '1s/^libsqlite3-sys //p' )
count=$( cd "$workspace" && cargo tree -p "$crate" --edges normal --prefix none --duplicates 2>/dev/null | grep -c '^libsqlite3-sys' || true )
if [ "${count:-0}" -gt 1 ]; then
  echo "::error::$crate links more than one libsqlite3-sys; the embedded build wants exactly one." >&2
  exit 1
fi

# E5 measures the embedded set from here: the count is cheap now and would not be later.
size=$(printf '%s\n' "$tree" | grep -c . || true)
echo "embedded deps: $crate's normal tree holds no axum, sqlx, hyper or tokio-postgres; one libsqlite3-sys ${version}; $size packages."
