# orch-store-sqlite

The SQLite implementation of the orchestrator's `ThreadStore` port: one file, one process, WAL, a
lock file, and every call on a blocking thread ([ADR 0076](../../../docs/decisions/0076-the-orchestrator-as-a-library-in-an-application.md)
decisions 3 and 6; epic [#382](https://github.com/vymalo/another-agentic-system/issues/382) E2).

## Where it sits

An **adapter** of the ports in [`orch-ports`](../ports/README.md), checked by that crate's
conformance testkit ([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)).
It is the store of a thread whose home is an application — a desktop, and later a phone — where the
orchestrator runs as a library and adam runs beside it in the same process. A deployment's thread
lives in Postgres ([`orch-store-postgres`](../store-postgres/README.md)) and never here: **a thread
has one home, fixed when it starts, and nothing else writes its log** (ADR 0076 decision 6). No
HTTP server, no Postgres and no `bin/orchestrator` depends on this crate; the library composition
root of ADR 0076 decision 4 will.

## What this milestone builds, and what it does not

| Port | State |
|---|---|
| `ThreadStore` — all 41 methods | **built**, and green on `thread_store_conformance!` (no skip) |
| `UsageLedger`, `ThreadSearch`, `PromptStore` | **declared, not built**: `ThreadStore`'s supertraits, so they must exist for it to compile. Every method answers `StoreError::Internal` (or `PromptStoreError::Internal`) naming milestone 2. Deliberately not an empty answer: an empty month, an empty result page and an empty library are things a caller cannot tell from "nothing was used", "nothing matched" and "nothing was saved" |
| `Wakeup`, `UserToolServerStore`, `PendingAuthorizations` | not in this crate yet |

The next milestones add the ledger, the index, the library and the rest, each with its own
migration, its own conformance suite, and the `forget` and `age` hooks those suites need (a store
that supplies no hook has those cases **skipped**, which is not green).

## One file, one process

```rust
use orch_store_sqlite::SqliteStore;

let store = SqliteStore::open("/home/me/.local/share/orch/orch.sqlite")?;
store.migrate().await?;
```

`open` takes an exclusive advisory lock on `<path>.lock` (an exclusive `flock`, named after the
file's real path, so a symlink and its target share it) and refuses a second process with
`StoreError::Refused("already_open")`. The operating system releases the lock when the process
exits, including a `SIGKILL`, so a restart opens the file at once. A `:memory:` database is not
offered: it has no lock and no WAL, so it is a store with none of the guarantees this crate exists
for.

The file is opened in WAL, with `foreign_keys=ON`, a five-second busy timeout and
`synchronous=NORMAL` (WAL's durable-against-a-crash setting: an application crash loses nothing, a
power loss may lose the last transactions).

## API at a glance

| Item | What |
|---|---|
| `SqliteStore::open(path)` | the store and its lock; `Result<_, StoreError>`. `Refused("already_open")` for a second process, `Internal` when the file or its lock cannot be opened or SQLite refuses a pragma, `Corrupt` when the file is not a database |
| `SqliteStore::migrate()` | applies `migrations/` in one `BEGIN IMMEDIATE` transaction, recording each file in `schema_migrations` (`version`, `description`, `checksum`, `applied_at`); idempotent; `Corrupt` when an applied file changed afterwards |
| `SqliteStore::path()` | the file (its real path) |
| `impl ThreadStore for SqliteStore` | the 41 methods of the port, with the contract of [`orch_ports::store`](../ports/src/store.rs): one transaction per write, the version check and the outbox and inbox fences inside it, the leases, the rail order, forks, child sessions, the purges, and the per-thread rules of the two claims |
| `schema::MIGRATIONS_TABLE` | `schema_migrations`. Not `sqlx`'s `_sqlx_migrations`: a SQLite file of this store has no history to continue, and naming the table after `sqlx` would claim one |

## The schema is not the Postgres one

The 29 migrations of `orch-store-postgres` are not portable: `jsonb`, `uuid`, `bytea`,
`array_to_tsvector`, partial indexes over expressions, `NOT VALID`. `migrations/0001_init.sql`
states the same tables in SQLite's types:

| Postgres | Here |
|---|---|
| `uuid` | `TEXT`, the canonical lowercase hyphenated form: it sorts bytewise exactly as the 16 bytes do, so `ORDER BY id` is the order of the ids and UUIDv7 ids sort by creation time |
| `timestamptz` | `INTEGER`, microseconds since the Unix epoch. A time is rounded to the microsecond before it is written, so what is read is what was written |
| `jsonb` | `TEXT` (`json_extract` reads a member) |
| `bytea` | `BLOB` |
| `bigserial ord` | `INTEGER`, from one counter read in the writing transaction: one writer, so no gaps and no ties |

The tables are `threads`, `events`, `a2a_bindings`, `outbox`, `watches`, `inbox` and
`thread_purges`, with the CHECKs Postgres has where SQLite can state them: the state, the event
kinds (the closed enum, ADR 0004), the outbox kinds and statuses, the inbox kinds and statuses, the
fork shape, the share shape (a thread is private exactly when it has no nonce), the child-session
pair and its 512-character bound, the description's 500, and the rule that an open outbox row has
an owner. `child_of` is a `DEFERRABLE INITIALLY DEFERRED` reference, because `delete_threads`
erases a parent and its named children in one statement.

Two Postgres features became this crate's job:

* **`outbox.owner`** is written by the insert (Postgres fills it with a `BEFORE INSERT` trigger, for
  replicas of an older build during a rollout; there is no rollout of this store).
* **The advisory lock per watch key** that serialises "add a watch" against `park_inbox` is gone:
  both are statements of one writer, and nothing else is in the file between them.

## Concurrency

`rusqlite` is synchronous. Every call runs on a blocking thread
([`tokio::task::spawn_blocking`]) and takes the one connection behind a **semaphore of one**, so the
async runtime never waits on the file and the calls of one process serialise instead of racing for
the write lock. The permit is taken on the runtime *before* the blocking task starts, so a queued
call never occupies a blocking-thread-pool worker while it waits.

Each change is one `BEGIN IMMEDIATE` transaction: the version check, the outbox and inbox fences,
the events, the job, the binding, the watches, the timers and the fences' completions are all in it,
so a crash leaves none of them. `FOR UPDATE`, `FOR SHARE` and `SKIP LOCKED` have no work to do in
one writer and are not written; the guarantees they carried hold because no second writer can be in
the file.

Errors follow the port's rule (ADR 0009: no driver type in a signature): the driver's error is the
`source` of `StoreError`. `SQLITE_BUSY` and `SQLITE_LOCKED` are `Unavailable` (a lock that outlived
the timeout: retry), a row that does not read as the port's type or a file that is not a database is
`Corrupt`, and a statement SQLite rejects is `Internal`. A unique-constraint failure is read by the
statement that can hit one, because what it means depends on the index.

## The bundled SQLite

`rusqlite` is pinned to **0.41.1** with `bundled`, which is the minor adam-rs's `adam-store-sqlite`
takes at our pin `7ac3281` (`libsqlite3-sys` 0.39.1, adam ADR 0053 decision 2), so an application
that links both has **one `sqlite3` and one version of it**. That build bundles **SQLite 3.54.1**,
which is at or after 3.35, the version `DELETE … RETURNING` needs (`PendingAuthorizations::take`,
a later milestone of this crate) and that this store's `UPDATE … RETURNING` and `AS MATERIALIZED`
already run on. `the_bundled_sqlite_is_at_least_three_thirty_five` in `src/schema.rs` is the proof,
and a bump of `rusqlite` that lowered it fails that test rather than a statement in production.

## Tests

```sh
cargo nextest run -p orch-store-sqlite
```

`tests/conformance.rs` wires `orch_ports::thread_store_conformance!` with a `make` that returns
`Some`: every case runs, none is skipped. Each case gets a store of its own on a file of its own in
the process's temporary directory, so the suite needs **no docker, no Postgres and no
`ORCH_TEST_DATABASE_URL`**. The crate's own tests cover what the testkit cannot: a second process
refused (including through a symlink, and the lock released when the store is dropped), WAL on,
the migrations recorded once, an edited migration refused, the bundled version, and the time
round-trip.

## Environment variables

None. The file's path is the caller's; nothing here reads the environment.
