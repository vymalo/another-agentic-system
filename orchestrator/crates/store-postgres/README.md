# orch-store-postgres

The Postgres implementation of the orchestrator's `ThreadStore` and `Wakeup`
ports (sqlx, `LISTEN/NOTIFY`, embedded migrations).

## Where it sits

An **adapter** of two ports in [`orch-ports`](../ports/README.md), checked by
that crate's conformance testkit
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)).
Postgres holds the only persistent state of the orchestrator: the thread
event log, the outbox and the inbox
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
Only the binary ([`orchestrator`](../../bin/orchestrator/README.md)) depends on
it.

## API at a glance

| Item | What |
|---|---|
| `PgStore::connect(url)`, `connect_with(url, max_connections)`, `from_pool(pool)`, `pool()` | construct; `Result<_, StoreError>` |
| `PgStore::migrate()` | applies the embedded migrations (`migrations/`: `0001_init.sql`; `0002_ui_events.sql`, which widens the `events.kind` check to the additive kinds `ui_surface` and `ui_action`; and `0003_job_ledger.sql`, which adds `threads.job jsonb NOT NULL DEFAULT '{}'`, widens the state and event-kind checks to `verifying` and `ci_result`, `check_result`, `rework`, and gives `outbox` the kind `verify` and a nullable `task_id` (used since slice 10: the verifier's A2A task, set by `mark_verify_sent`); and `0004_inbox.sql`, which adds `inbox` (`UNIQUE (source, idempotency_key)`; a partial index for the pending rows in due order, one for the inflight rows by lease, and two for the parked rows, by correlation and by age) and `watches` (`key` primary key, `thread_id` referencing `threads`); the inbox has `attempts`, the fencing token, and `refunded`, the claims that do not count against the attempt limit); sqlx records them under an advisory lock, so every replica may run it at boot |
| `impl ThreadStore for PgStore` | per-thread `seq` from a counter row updated in the same transaction as the event insert (no gaps, no duplicates); optimistic `version`; outbox claims with `FOR UPDATE SKIP LOCKED` and leases (a `cancel` or `verify` row is claimable whatever the thread's older delegations; a delegation waits for the older ones of its thread); every outbox write matches `id`, owner, `attempts` (the fencing token) and `status = 'inflight'`, and `commit` checks its `Commit.lease` with a `FOR SHARE` lock on the outbox row right after the thread lock, answering `CommitOutcome::Fenced` (no migration); the job is written in the same `UPDATE` as the state (`job = COALESCE($n, job)`), so it shares the row lock, the version check and the idempotency check of the commit; `outbox_stats` is one aggregate query over the open rows (the `outbox_open` partial index), with the `claim_outbox` due predicate; the inbox: `claim_inbox` is one `WITH .. FOR UPDATE SKIP LOCKED` statement (`pending` and `available_at <= now`, or `inflight` and `lease_until <= now`; earliest first), every write matches `id`, owner, `attempts` and `status = 'inflight'`, and `commit` checks its `Commit.inbox` with a `FOR UPDATE` lock on the row (thread lock first, then the outbox row, then the inbox row) and marks it `applied` in the same transaction (a commit that carries nothing else and the thread's own state leaves the thread alone: after the same claim and version checks it only marks the row); `release_inbox_leases` and a park add the claim to `refunded`; a watch is inserted `ON CONFLICT DO NOTHING` (a warning names both threads when another one already owns the key) and the `parked` rows with its correlation are set `pending` (and refunded) by the same transaction, selected in id order with `FOR UPDATE SKIP LOCKED`, as `expire_parked_inbox` selects the rows it expires, so neither waits for a row lock; a timer is an `INSERT .. ON CONFLICT (source, idempotency_key) DO NOTHING` due at the commit's `now` plus `after`. Adding a watch and `park_inbox` take one transaction-scoped advisory lock per watch key (`pg_advisory_xact_lock(hashtextextended('orch:watch:<key>', 0))`, keys sorted) and `park_inbox` looks for the watch again under it, so a row is never left parked behind a watch that a concurrent commit just added. That does not make deadlock impossible (a stale `park_inbox` holds a key and waits for a row that the commit of the worker that took the row over holds, while that commit wants the key): Postgres detects it, aborts one transaction (`40P01`), and the store reports a transient error the caller retries. The payload is read as JSON and decoded by the caller |
| `PgWakeup::start(pool)`, `wait_listening(timeout)` | `LISTEN/NOTIFY` fan-out (channels `orch_thread`, `orch_outbox`, `orch_inbox` and `orch_resync`; `receive` and a commit or park that makes a row claimable send `orch_inbox` inside their transaction); a reconnect or a lagging subscriber yields `Topic::Resync` |

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
* `tests/postgres.rs`: behaviour specific to this implementation, including that migration 0003 upgrades a
  database that ran 0001 and 0002 and holds a thread (its job reads back as the default, the widened
  constraints take the new values and still refuse others); that migration 0004 upgrades one that ran
  0001 to 0003 (the new tables take rows, refuse a repeated key, an unknown kind and an unknown status,
  and a deleted thread takes its watches); that receiving and re-arming wake a subscriber in another
  process; that a park waits for a commit that is adding its watch and then puts the row back, and that
  a commit adding a watch waits for a park and then re-arms the row it set aside (each holds the advisory
  lock in a transaction of the test, proves from `pg_locks` that the other call is blocked on it, and
  then lets go: no timing decides the outcome; each failed with the lock taken out, 2026-09-30); that
  expiry passes by a row another transaction holds instead of waiting; and that a row whose payload this build cannot read is
  claimed with its neighbours and fails alone when decoded.

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
