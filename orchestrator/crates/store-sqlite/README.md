# orch-store-sqlite

The SQLite implementation of the orchestrator's `ThreadStore` port and of the two supertraits it
builds — `UsageLedger` (ADR 0065) and `ThreadSearch` (ADR 0068): one file, one process, WAL, a
lock file, and every call on a blocking thread
([ADR 0076](../../../docs/decisions/0076-the-orchestrator-as-a-library-in-an-application.md)
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
| `ThreadStore` — all 43 methods | **built**, and green on `thread_store_conformance!` (no skip); the thread's default model and reasoning effort ([ADR 0086](../../../docs/decisions/0086-a-person-picks-a-model-or-a-reasoning-level-for-a-thread-or-a-message.md)) is `threads.pick`, migration `0004_thread_pick.sql` |
| `UsageLedger` (ADR 0065) | **built** (milestone 2a, migration `0002`), green on `usage_ledger_conformance!` with its `forget` hook (no skip) |
| `ThreadSearch` (ADR 0068) | **built** (milestone 2a, migration `0002`), green on `thread_search_conformance!` with its `forget` and `age` hooks (no skip) |
| `PromptStore` (ADR 0070) | **built** (milestone 2b, migration `0003`), green on `prompt_store_conformance!` (no skip; the suite needs no hook) |
| `Wakeup` | **built** (`SqliteWakeup`, in-process broadcast channels: one file belongs to one process, so no `LISTEN`/`NOTIFY`), green on `wakeup_conformance!` (no skip) |
| `UserToolServerStore`, `PendingAuthorizations` | not in this crate yet |

The prompt library (`src/prompts.rs`, `migrations/0003_user_prompts.sql`) mirrors
[`store-postgres/src/prompts.rs`](../store-postgres/src/prompts.rs): one transaction per write in place
of the owner's advisory lock, one counter for revisions, the audience as a JSON array matched with
`json_each`. `SqliteWakeup` is in-process broadcast channels (`src/wakeup.rs`).

## The ledger and the index

`migrations/0002_usage_and_search.sql` adds the usage ledger of the people and the search index of
their threads. Both are **a function of the log**, written by `ThreadStore::commit` and by the
creation of a thread **in the transaction that appends the event**, so they cannot disagree with it
(ADR 0065 decision 4, ADR 0068 decision 6):

| Postgres (`0023`, `0025`) | Here (`0002`) |
|---|---|
| `usage_tasks`, `usage_lines`, `usage_sandbox` | the same three tables in SQLite's types; `usage_tasks.id` is the rowid (`INTEGER PRIMARY KEY`), not an identity sequence |
| `threads.usage_through`, `search_through`, `search_fold` | the same three columns |
| `message_terms(…, terms tsvector)` | `message_terms(thread_id, seq, owner, message_id, key)`: one row per distinct folded key of a message, cascading from `events` |
| `threads.title_terms`, `description_terms` (`tsvector`) | `thread_terms(thread_id, kind, key)`: one row per distinct key, `kind` being `title` or `description`, cascading from `threads` |

**Not FTS5.** FTS5 is not compiled into every bundled SQLite, and its folding is not
`orch_core::search::fold`'s. A store's job (see the module doc of `crates/core/src/search.rs`) is to
hold the keys `document_keys` makes and to answer the keys of a query, so the candidates are the
contract's whatever the database's collation or text-search configuration. The keys are already
folded and are matched **by prefix** with `substr(key, 1, length(term)) = term`, a byte comparison;
a query's text never reaches SQL, only the terms of a `SearchTerms`.

The title's and description's keys live in a table rather than in columns because Postgres has an
array type (`tsvector`) and SQLite does not: a JSON column would put `json_each` in every query and
make "replace the keys" a rewrite of a document. A table of key rows gives **one predicate for
every document** (title, description, message: every term has a key row that starts with it), one
`DELETE` for the erase and one for the conformance hook.

What the writes do:

* **`commit`** first catches the ledger up with what it has not been through (every commit; the
  watermark is contiguous), then folds this commit's events into the ledger. For the index it
  catches up **at most `COMMIT_CATCH_UP_EVENTS` (500) old events** per commit, advancing the
  watermark only as far as it got, then indexes its own events (rows above the watermark; the
  writes are upserts, so the later catch-up that reaches them changes nothing). A thread stored
  under another `FOLD_VERSION` is deleted and indexed again from the start. A title or description
  the commit writes replaces that document's keys.
* **`create_thread` / `fork_thread`** fold and index the first commit's events. A fork's copied
  events are indexed **from the fork's own log** (the parent's rows may no longer hold a message the
  parent replaced), while the ledger counts only the fork's own events — what it copied was counted
  in the parent.
* **`delete_threads`** clears `usage_tasks.thread_id` of the erased threads in the same transaction
  (the usage stays, the link goes) and leaves the forks' rows alone; the index rows go with
  `ON DELETE CASCADE` from `events` and `threads`.
* A **refused** commit (a version conflict, a lost claim, a repeated key) writes neither, because
  every check runs before the first write and the whole commit is one transaction.

The backfill of each port pages through the threads in id order, one transaction per thread, and is
idempotent. `UsageLedger::month` sums with `UsageMonthTotals::of`, the function the in-memory store
uses, so the two cannot disagree on the parts, the sandbox shapes or the order; the usage backfill
never waits for a thread's lock (one writer), so `skipped` is always 0 here.

## Conformance hooks

`SqliteStore::forget_usage_ledger`, `forget_search_index` and `age_search_index` make the store
behave as if the log predated the ledger or the index, or as if every thread had been indexed under
an older fold version. They are **test hooks, not production paths**: only the `forget` and `age`
arguments of the conformance macros call them. Without them the cases that play an older log are
skipped, and a skipped case is not a passing one.


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
| `SqliteStore::forget_usage_ledger()`, `forget_search_index()`, `age_search_index()` | the conformance hooks (see below): **test only**, never a production path |
| `impl ThreadStore for SqliteStore` | the 41 methods of the port, with the contract of [`orch_ports::store`](../ports/src/store.rs): one transaction per write, the version check and the outbox and inbox fences inside it, the leases, the rail order, forks, child sessions, the purges, and the per-thread rules of the two claims; the commit also writes the ledger and the index |
| `impl UsageLedger for SqliteStore` | `month`, `thread_rows`, `backfill`, `sweep` (ADR 0065) |
| `impl ThreadSearch for SqliteStore` | `search_threads`, `backfill_search`, `search_backlog` (ADR 0068) |
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

`migrations/0002_usage_and_search.sql` adds the ledger's three tables and the index's two, with the
month's shape, the source, the non-negative counts, the sandbox shape and the rule that a measured
sandbox names an environment; see *The ledger and the index* above.

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
the events, the job, the binding, the watches, the timers, the fences' completions, the ledger's
rows and the index's keys are all in it, so a crash leaves none of them. `FOR UPDATE`, `FOR SHARE`
and `SKIP LOCKED` have no work to do in one writer and are not written; the guarantees they carried
hold because no second writer can be in the file.

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

`tests/conformance.rs` wires **every** suite (`thread_store`, `usage_ledger`, `thread_search`, `prompt_store`, `wakeup`) with a `make` that returns `Some`:
`thread_store_conformance!`, `usage_ledger_conformance!` with its `forget` hook and
`thread_search_conformance!` with its `forget` and `age` hooks. Every case runs, none is skipped
(208 cases, `0 skipped`). Each case gets a store of its own on a file of its own in the process's
temporary directory, so the suite needs **no docker, no Postgres and no
`ORCH_TEST_DATABASE_URL`**. The crate's own tests cover what the testkit cannot: a second process
refused (including through a symlink, and the lock released when the store is dropped), WAL on,
the migrations recorded once, an edited migration refused, the bundled version, the time
round-trip, and the shapes of the search predicates and of a page's parameters.

## Environment variables

None. The file's path is the caller's; nothing here reads the environment.
