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
| `PgStore::migrate()` | applies the embedded migrations (`migrations/`: `0001_init.sql`; `0002_ui_events.sql`, which widens the `events.kind` check to the additive kinds `ui_surface` and `ui_action`; and `0003_job_ledger.sql`, which adds `threads.job jsonb NOT NULL DEFAULT '{}'`, widens the state and event-kind checks to `verifying` and `ci_result`, `check_result`, `rework`, and gives `outbox` the kind `verify` and a nullable `task_id` (used since slice 10: the verifier's A2A task, set by `mark_verify_sent`); and `0004_inbox.sql`, which adds `inbox` (`UNIQUE (source, idempotency_key)`; a partial index for the pending rows in due order, one for the inflight rows by lease, and two for the parked rows, by correlation and by age) and `watches` (`key` primary key, `thread_id` referencing `threads`); the inbox has `attempts`, the fencing token, and `refunded`, the claims that do not count against the attempt limit; and `0005_job_started.sql`, which widens the `events.kind` check to `job_started`, ADR 0020; an outbox `cancel` row written before it named its job is the bare string `"cancel"` and reads as the current job's; and `0006_ui_catalog.sql`, which widens the `events.kind` check to `ui_catalog`, ADR 0023: the thread's catalog ledger lives inside `threads.job` and the delivery inside the outbox payloads, so no column is added; and `0007_agent_step.sql`, which widens the `events.kind` check to `agent_step`, ADR 0025: the ledger of open steps lives inside `threads.job`, so no column is added; and `0008_thread_titled.sql`, which widens the `events.kind` check to `thread_titled`: the title's source and ask ledger live inside `threads.job` and the title in the existing `threads.title`, so no column is added; and `0009_title_requests.sql`, which widens the `outbox.kind` check to `title` (added `NOT VALID` and then validated, so the table lock is brief): a `title` row is the one thing an older build cannot read, so roll out the build that understands it first); and `0010_thread_forks.sql`, which adds `threads.forked_from` (`REFERENCES threads ON DELETE SET NULL`), `forked_at` and `fork_kind` (`fork` or `edit`; all `NULL` for a thread that was not forked, checked together, added `NOT VALID` and validated) with an index on `forked_from`, and widens the `events.kind` check to `thread_forked`, ADR 0029; `0011_thread_description.sql`, which adds `threads.description` (`NULL` when the thread has none, at most 500 characters and never empty: `threads_description_len`, added `NOT VALID` and validated), widens the `events.kind` check to `thread_described` and the `outbox.kind` check to `description`, ADR 0035); sqlx records them under an advisory lock, so every replica may run it at boot |
| `impl ThreadStore for PgStore` | per-thread `seq` from a counter row updated in the same transaction as the event insert (no gaps, no duplicates); optimistic `version`; outbox claims with `FOR UPDATE SKIP LOCKED` and leases (a `cancel`, `verify` or `title` row is claimable whatever the thread's older delegations; a delegation waits for the older ones of its thread); every outbox write matches `id`, owner, `attempts` (the fencing token) and `status = 'inflight'`, and `commit` checks its `Commit.lease` with a `FOR SHARE` lock on the outbox row right after the thread lock, answering `CommitOutcome::Fenced` (no migration); `Commit.finishes_outbox` finishes the claimed outbox row (the row already locked `FOR SHARE` for the fence) in the same transaction, with the same outcomes as `complete_outbox`; the job is written in the same `UPDATE` as the state (`job = COALESCE($n, job)`), so it shares the row lock, the version check and the idempotency check of the commit; `outbox_stats` is one aggregate query over the open rows (the `outbox_open` partial index), with the `claim_outbox` due predicate; the inbox: `claim_inbox` is one `WITH .. FOR UPDATE SKIP LOCKED` statement (`pending` and `available_at <= now`, or `inflight` and `lease_until <= now`; earliest first), every write matches `id`, owner, `attempts` and `status = 'inflight'`, and `commit` checks its `Commit.inbox` with a `FOR UPDATE` lock on the row (thread lock first, then the outbox row, then the inbox row) and marks it `applied` in the same transaction (a commit that carries nothing else and the thread's own state leaves the thread alone: after the same claim and version checks it only marks the row); `release_inbox_leases` and a park add the claim to `refunded`; a watch is inserted `ON CONFLICT DO NOTHING` (a warning names both threads when another one already owns the key) and the `parked` rows with its correlation are set `pending` (and refunded) by the same transaction, selected in id order with `FOR UPDATE SKIP LOCKED`, as `expire_parked_inbox` selects the rows it expires, so neither waits for a row lock; a timer is an `INSERT .. ON CONFLICT (source, idempotency_key) DO NOTHING` due at the commit's `now` plus `after`. Adding a watch and `park_inbox` take one transaction-scoped advisory lock per watch key (`pg_advisory_xact_lock(hashtextextended('orch:watch:<key>', 0))`, keys sorted) and `park_inbox` looks for the watch again under it, so a row is never left parked behind a watch that a concurrent commit just added. That does not make deadlock impossible (a stale `park_inbox` holds a key and waits for a row that the commit of the worker that took the row over holds, while that commit wants the key): Postgres detects it, aborts one transaction (`40P01`), and the store reports a transient error the caller retries. The payload is read as JSON and decoded by the caller |
| `PgStore::fork_thread` and `fork_family` | a fork is `create_thread` with the copy in its transaction: the parent is read `FOR KEY SHARE` (so it cannot be deleted under the copy while its own commits go on) and must be the owner's and reach the cut; the thread row is inserted with `last_seq = cut` and its origin; the events are copied by one `INSERT .. SELECT` of `seq <= cut` (the statement must copy exactly `cut` rows, else the transaction is rolled back as `Corrupt`); the first commit's events follow from `cut + 1`. `fork_family` is one recursive query (up the `edit` links, then down them, `LIMIT 1000`) with the seq of each edit's replacing message found by a subquery on the events. `list_threads` leaves out `fork_kind = 'edit'` unless asked |
| `PgWakeup::start(pool)`, `wait_listening(timeout)` | `LISTEN/NOTIFY` fan-out (channels `orch_thread`, `orch_outbox`, `orch_inbox` and `orch_resync`; `receive` and a commit or park that makes a row claimable send `orch_inbox` inside their transaction); a reconnect or a lagging subscriber yields `Topic::Resync`. **Live text** ([ADR 0027](../../../docs/decisions/0027-live-text-relayed-not-stored.md)) goes on a fifth channel, `orch_live`, over the same listener, with its own broadcast: `publish_live` sends one `pg_notify` per payload, outside any transaction of the store and never stored, with the JSON `{"t": thread, "a": agent, "m": stream id, "o": UTF-8 byte offset, "x": text, "e": "o" \| "l" \| "a"}`. A `NOTIFY` carries less than 8000 bytes (*verified 2026-10-01*, [postgresql.org/docs/16/sql-notify.html](https://www.postgresql.org/docs/16/sql-notify.html)), so a payload is at most 7900 and a piece whose JSON is longer (text full of quotes, newlines or control characters escapes to up to six times its size) is split at character boundaries into several, in order, each with its own offset and the piece's end on the last one only; only an envelope that cannot fit at all (a stream id of kilobytes) is `WakeupError::PayloadTooLarge`. A payload this build cannot read is dropped (`debug` log), and neither a reconnect of the listener nor a lagging subscriber sends a `Resync` for live text: it is best effort, the sender repeats the text so far |

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
  `wakeup_conformance!`, live cases included) against Postgres. The unit tests of `src/wakeup.rs` pin the payload codec: the documented JSON, a round trip, what this build does not write is not live text, and that no payload reaches the limit whatever the text is made of (every character escaped) while the parts rejoin by offset into the text. `ui_catalog_event(thread, digest)` is `WHERE kind = 'ui_catalog' AND data ->> 'digest' = $2 ORDER BY seq DESC LIMIT 1` (a thread has few such events, so it needs no index of its own); the case `ui_catalog_event_by_digest` finds an event behind 70 others.
* `tests/postgres.rs`: migration 0011 on a database that ran 0001 to 0010 and holds a log and a `title` row (the old rows stay and the old thread has no description, the old constraints refuse a `thread_described` event and a `description` row, the new ones take them and still refuse an unknown kind, a description the core wrote reads back, and the column refuses an empty or an over long one); the store conformance cases `thread_described_roundtrip` and `a_fork_starts_with_the_description_it_is_given` run here too.
* `tests/postgres.rs` (the fork tests): migration 0010 on a database that holds a log (the old thread reads as not forked, the old constraint refuses `thread_forked`, the new one takes it and still refuses an unknown kind, and the fork columns refuse half an origin, an unknown kind and a negative cut), a fork that outlives the deletion of its parent (its copy of the log is whole, its origin names no thread, and its family starts at it), and sixteen-way forks made while the parent is written to, each holding exactly the events up to its cut.
* `tests/postgres.rs`: behaviour specific to this implementation, including that live text published by one replica reaches a subscriber of another (and a stray payload on the channel is dropped without disturbing what follows), works again after the listener's connection is killed (`live_text_works_again_after_a_listener_reconnect`; the hints get their `Resync`), and that a subscriber that did not read while 2000 pieces arrived loses the oldest without a word and still gets the newest and the next; including that a legacy bare `"cancel"` outbox row reads as `Cancel { job: None }` and `Cancel { job: Some(n) }` round-trips, including that migration 0003 upgrades a
  database that ran 0001 and 0002 and holds a thread (its job reads back as the default, the widened
  constraints take the new values and still refuse others); that migration 0004 upgrades one that ran
  0001 to 0003 (the new tables take rows, refuse a repeated key, an unknown kind and an unknown status,
  and a deleted thread takes its watches); that migration 0007 upgrades one that ran 0001 to 0006 and
  holds a log (the old event stays, the old constraint refuses an `agent_step`, the new one takes it and
  still refuses an unknown kind), and migration 0008 the same for a `thread_titled` on one that ran 0001 to 0007, and migration 0009 for a `title` outbox row on one that ran 0001 to 0008 and holds a log (the old rows stay, the old constraint refuses a `title` row, the new one takes it and still refuses an unknown kind); that receiving and re-arming wake a subscriber in another
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
