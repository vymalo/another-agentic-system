#!/usr/bin/env sh
# sqlx stays out of the orchestrator's workspace (ADR 0076, sys#382 E1): every Postgres crate is on
# tokio-postgres through orch-pg-connect and orch-pg-migrate, and nothing may bring sqlx back, not
# even as a dev-dependency or through another crate's dependencies. The lockfile is the whole tree.
#
# Usage: tools/sqlx-out-check.sh [path to Cargo.lock]     (default: orchestrator/Cargo.lock)
# Exit 0 = no package named sqlx or sqlx-*; exit 1 = one is there (each is named).

set -eu

LOCK=${1:-orchestrator/Cargo.lock}

if [ ! -f "$LOCK" ]; then
  echo "::error::$LOCK does not exist." >&2
  exit 1
fi

found=$(sed -n 's/^name = "\(sqlx\(-[^"]*\)\{0,1\}\)"$/\1/p' "$LOCK" | sort -u)
if [ -n "$found" ]; then
  echo "::error::$LOCK holds sqlx again; the orchestrator's Postgres crates use tokio-postgres (orch-pg-connect, orch-pg-migrate):" >&2
  echo "$found" >&2
  echo "Find what brings it in with: (cd orchestrator && cargo tree -i <package> --workspace --all-features --edges all)" >&2
  exit 1
fi
echo "sqlx out: $LOCK holds no sqlx package."
