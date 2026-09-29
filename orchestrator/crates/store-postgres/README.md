# orch-store-postgres

The Postgres implementation of the orchestrator's `ThreadStore` and `Wakeup`
ports (sqlx, `LISTEN/NOTIFY`, embedded migrations).

## Where it sits

An **adapter** of two ports in [`orch-ports`](../ports/README.md), checked by
that crate's conformance testkit
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)).
Postgres holds the only persistent state of the orchestrator: the thread
event log and the outbox
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
Only the binary ([`orchestrator`](../../bin/orchestrator/README.md)) depends on
it.

## API at a glance

| Item | What |
|---|---|
| `PgStore::connect(url)`, `connect_with(url, max_connections)`, `from_pool(pool)`, `pool()` | construct; `Result<_, StoreError>` |
| `PgStore::migrate()` | applies the embedded migrations (`migrations/`); sqlx records them under an advisory lock, so every replica may run it at boot |
| `impl ThreadStore for PgStore` | per-thread `seq` from a counter row updated in the same transaction as the event insert (no gaps, no duplicates); optimistic `version`; outbox claims with `FOR UPDATE SKIP LOCKED` and leases; every outbox write matches `id`, owner, `attempts` (the fencing token) and `status = 'inflight'`, and `commit` checks its `Commit.lease` with a `FOR SHARE` lock on the outbox row right after the thread lock, answering `CommitOutcome::Fenced` (no migration); `outbox_stats` is one aggregate query over the open rows (the `outbox_open` partial index), with the `claim_outbox` due predicate |
| `PgWakeup::start(pool)`, `wait_listening(timeout)` | `LISTEN/NOTIFY` fan-out; a reconnect or a lagging subscriber yields `Topic::Resync` |

```rust
use orch_store_postgres::{PgStore, PgWakeup};

let store = PgStore::connect(url).await?;
store.migrate().await?;
let wakeup = PgWakeup::start(store.pool().clone());
```

Only runtime-checked `sqlx::query` is used, so building needs no database.
Applied migrations are append-only: never edit one, add a new file (CI checks
this against the base branch).

## Features and environment

No Cargo features. The crate reads no environment variables.

## Tests

Gated on one variable; without it the database tests print a notice and pass
without running.

| Variable | Meaning |
|---|---|
| `ORCH_TEST_DATABASE_URL` | a Postgres to test against, e.g. `postgres://postgres:postgres@localhost:5432/orch_test` (`docker compose up -d --wait postgres` creates that database; see [`dev/README.md`](../../../dev/README.md)) |

Each test gets its own schema (`search_path`), so tests run in parallel on one
database; stale test schemas older than an hour are dropped.

* `tests/conformance.rs`: the `orch-ports` testkit (`thread_store_conformance!`,
  `wakeup_conformance!`) against Postgres.
* `tests/postgres.rs`: behaviour specific to this implementation.

```sh
ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test \
  cargo test -p orch-store-postgres
```

CI sets the variable against a Postgres service
([`orchestrator.yml`](../../../.github/workflows/orchestrator.yml)). The same
variable enables the Postgres variants in `orch-e2e` and the `orchestrator`
binary's smoke test.

## See also

[`orch-ports`](../ports/README.md), [`orch-e2e`](../e2e/README.md).
