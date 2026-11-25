# orch-agent-adam

Local agents for the orchestrator ([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)):
`AgentClient` over adam-rs agents hosted in the orchestrator's own process, with their journal in the
orchestrator's Postgres.

## Where it sits

An **adapter** of the `AgentClient` port in [`orch-ports`](../ports/README.md), next to
[`orch-agent-a2a`](../agent-a2a/README.md). A remote agent stays a plain A2A agent behind that crate; this one
is the other way to reach an agent, with no card URL and no network hop, for an `AGENTS_FILE` entry
with `transport: local`. It is the **only** crate of the workspace that depends on adam-rs's runtime, A2A
task backend and store crates (git dependencies pinned to one commit sha in `orchestrator/Cargo.toml`), and the
[binary](../../bin/orchestrator/README.md) links it only with its Cargo feature `agent-local`, off by default
([ADR 0007](../../../docs/decisions/0007-protocol-only-dependencies.md), amended by ADR 0015). No adam type appears
in a signature that the ports see.

It drives one `adam_runtime::Runtime` per process through the A2A `TaskBackend` seam
(`adam-a2a-runtime`'s `RuntimeTaskBackend`) and maps the results with
[`orch-a2a-mapping`](../a2a-mapping/README.md), the mapping `orch-agent-a2a` uses, so the idempotency keys of an
envelope are the HTTP adapter's. Design, with its two diagrams: [`docs/orchestrator.md`](../../../docs/orchestrator.md#local-agents).

## API at a glance

| Item | What |
|---|---|
| `LocalAgents::postgres(pool, listen_url, LocalOptions, &[LocalKind])` | the agents over a `deadpool_postgres::Pool`: the store `adam-store-postgres` with the prefix `TABLE_PREFIX`, the notifier `adam-notify-postgres` on the channels of the same prefix (none when `steps` is false). The notifier sends its `NOTIFY`s on `pool` and listens on a connection of its own to `listen_url`, the same database (a pooled connection never receives a notification); adam-rs's `adam-postgres` reads that string, because `PgNotify` takes nothing else, and it reads the URL alone: no `PG*` variable and no password file. So a `listen_url` that leaves a part of the server to them (`orch-pg-connect`'s `left_to_the_environment`: the password in `PGPASSWORD`, the user from the operating system, ...) is refused as `Invalid`, naming the part, where the pools would connect and the listener never would. It is built with `local_only(false)`: events cross processes whatever adam-rs's `NOTIFY_LOCAL_ONLY` says. Fails on an invalid prefix, or a `listen_url` that does not parse or leaves a part to the environment |
| `LocalAgents::postgres_direct(pool, direct, direct_url, LocalOptions, &[LocalKind])`, `DIRECT_CONNECTIONS` (2) | `postgres` for a `pool` that goes through a transaction-mode pooler (PgBouncer): the journal's queries use `pool`; the notifier (its `NOTIFY`s on `direct`, its `LISTEN` on a connection of its own to `direct_url`) and the creation of the tables (`migrate`) take `direct`, a pool of connections to the database itself. adam-rs's migration holds a **session** advisory lock of the prefix on one connection (`pg_advisory_lock(hashtext(...))`, on `tokio-postgres` since adam-rs `7ac3281`), which a pooler cannot keep, so it needs the direct pool; so does the `LISTEN` |
| `LocalAgents::connect(database_url, direct_url, LocalOptions, &[LocalKind])` | builds the pool itself and then `postgres`: `concurrency + 3` connections, separate from the orchestrator's, plus the notifier's listener; a database that cannot be reached is a transient `LocalAgentsError`. With `Some(direct_url)` the pool has `concurrency + 2` connections, and `postgres_direct` gets a second pool of `DIRECT_CONNECTIONS` to `direct_url` (and the listener's connection to it): the same counts as before the listener left the pool. The pools are `orch-pg-connect`'s (the connection string and TLS of the rest of the orchestrator; `tokio-postgres` sends no startup parameter but the URL's own, so no `extra_float_digits`, which PgBouncer refuses), each waits at most 30 s for a connection (sqlx's `acquire_timeout`), and one connection of each is made now. A connection string that does not parse is refused without its text |
| `LocalAgents::memory(LocalOptions, &[LocalKind])` | the same over an in-memory journal: nothing survives the process, no other process sees it |
| `migrate()` | creates the tables if missing; idempotent, safe from every replica at once (an advisory lock of the prefix) |
| `client() -> LocalAgentClient` | the `AgentClient`; `LocalAgentClient::default()` hosts nothing |
| `run(CancellationToken)` | the worker and the notifier together, until cancelled; if one ends by itself the other is stopped and the first error returned. With `steps: false` it only waits |
| `LocalKind` | the closed set of kinds this crate hosts: `Echo` (repeats the message back; needs no model). `name()`, `description()`, `ALL` |
| `LocalOptions` | `instance_id` (the worker id in run leases: unique per process), `concurrency`, `lease_ttl`, `poll_interval`, `steps`; `LocalOptions::new(id)` has 4, 30 s, 250 ms, `true` |
| `TABLE_PREFIX` | `"orch_agent_"`: the tables `orch_agent_runs`, `orch_agent_journal`, `orch_agent_meta` (and, since adam-rs's schema version 6, `orch_agent_push`, `orch_agent_blob`, `orch_agent_run_blob`, `orch_agent_body`) and the `NOTIFY` channels `orch_agent_events`, `orch_agent_signals` |
| `LocalAgentsError` | `thiserror`; keeps its source; implements `orch_core::Classify` with the lower error's class; `LocalAgentsError::unavailable(source)`. It never holds a connection string, nor a database's error as the driver keeps it: where its chain reaches `tokio-postgres`, the chain ends in that error's words (the server's severity, message and SQLSTATE, never its `DETAIL` or `HINT`, which may quote a row; or the driver's kind and the I/O error under it). The client's errors keep their sources the same way |

```rust
use orch_agent_adam::{LocalAgents, LocalKind, LocalOptions};
use tokio_util::sync::CancellationToken;

let agents = LocalAgents::connect(&database_url, None, LocalOptions::new("replica-1"), &[LocalKind::Echo]).await?;
agents.migrate().await?;
let client = agents.client();                    // then PortSet { agents: ByTransport { a2a, local: client }, .. }
let stop = CancellationToken::new();
tokio::spawn(async move { agents.run(stop).await });
```

## How the client behaves

* **Endpoints.** It serves `AgentTransport::Local` only: an A2A endpoint is `Unsupported`; a local endpoint of a
  kind this process does not host is `Unreachable` (transient: another process may host it).
* **Caller and tasks.** The caller is `orch:<agent id>`, so two configured agents of one kind never see each
  other's tasks. A new task's id comes from `task_id_for(kind, caller, context, message id)`: sending the same
  message twice reaches one task, and `find_task_by_message` recomputes the id and answers `Some` only if that task
  exists. `SendRequest.context_id` is `None` for the first message of a conversation: the runtime starts one and the
  task names it ([ADR 0055](../../../docs/decisions/0055-the-agent-assigns-the-a2a-context.md)); the id of such a message is
  derived from no context, which is also how it is found again.
* **`send_stream`** submits the message (a follow-up to an `input-required` task when the request names a task; the
  message's `referenceTaskIds` are the request's, [ADR 0021](../../../docs/decisions/0021-context-across-a2a-tasks.md)),
  then subscribes: the first frame is a snapshot, then status and artifact events; the stream ends after the event
  that finishes the task or leaves it waiting for its caller, and after the first error. A stream that closes
  without an event is a `Protocol` error. A selected release, or an A2UI action, is `Rejected` (fail closed). A person's child-session start or close (`SendContent::SessionStart`, `SessionClose`, `child-sessions/v1`, ADR 0072) is `Unsupported`, nothing sent: a local agent's card lists no extension. A message that names an access tier (`SendRequest.tier`, ADR 0061) is `Unsupported`, nothing sent: the card of a local agent lists no tier, so the message would run under the agent's own default, and a tier is never dropped. A steer (`SendRequest.steer`, `steer/v1`, ADR 0036) is `Unsupported`: the card of a local agent lists no extension, so the dispatcher keeps the message and delivers it after the turn; the pinned adam-rs (`7ac3281`) already takes a message for a running task when the caller activates `steer/v1` (`RuntimeTaskBackend::submit`, `Caller::with_extensions`), but listing the extension is the host's promise that its agent reads an accepted message and never loses it, and the only local kind, `echo`, ends in one step without reading its inbox, so a steer stays a message sent after the turn until a local kind that reads its inbox (adam's `LlmAgent`, which asks `Ctx::reopen_on_arrival`) exists here.
* **`resubscribe`** works while the task is not finished, from any process; a finished or unknown task is
  `TaskNotFound` (the dispatcher polls `get_task`). `cancel` ends the run and reads back `canceled`; a finished
  task is `NotCancelable`.
* **Errors.** `TaskNotFound`, `NotCancelable`, `InvalidParams` (as `Rejected`) keep their meaning; an unavailable
  runtime is `Unreachable`, anything else `Protocol`, both with their source and neither leaking it to users.
* **`read_card`** needs no runtime: the kind's description, this crate's version, no releases, no UI, no access tier (`AgentCardInfo.access_tier` is `None`).

## Features and environment

| Feature | Default | Effect |
|---|---|---|
| `embedded` | no | Pulls `adam-engine` and `adam-store-sqlite` (one `libsqlite3-sys`) for the embedded build (ADR 0076, ADR 0015). Wiring only: no behaviour changes and nothing uses them yet |
| `testkit` | no | `testkit::{LocalFixture, LocalWorld, LocalInstance, scripted_endpoint, SCRIPTED}`: a scripted adam agent (words `echo`, `ask`, `gate`, `slow`, `fail`), the `AgentFixture` of the conformance suite, processes that share one journal (in memory, a private schema of the test database, or the database a URL names: `LocalWorld::postgres_over(url)`, and `postgres_direct_over(url, direct_url)` behind a pooler) and can be killed; `LocalWorld::url()` and `pool(name, max)` (a `deadpool_postgres::Pool`) for a test that looks at the tables and channels. Enable as a **dev-dependency** feature |

The crate reads no environment variable (adam-rs's `PgNotify` would read `NOTIFY_LOCAL_ONLY`; this crate sets it); the binary maps `AGENT_LOCAL_CONCURRENCY` and `OUTBOX_LEASE_SECS` onto
`LocalOptions`. The Postgres tests use `ORCH_TEST_DATABASE_URL` and skip without it. `tests/direct.rs` (issue 290) proves the routing of `connect` with a direct URL from the database's side (two URLs whose sessions are named by `application_name`; an event trigger records the session of every DDL statement: the tables are made by the direct pool; `pg_stat_activity`: one session listens and it is the direct one; an echo run completes), with or without a pooler (`ORCH_TEST_DIRECT_DATABASE_URL`). The other suites of this crate isolate by a schema (`search_path`), which a pooler cannot carry, so they run only against Postgres itself; behind a pooler the journal is exercised by the binary's `tests/local.rs` and by `orch-e2e`, whose `LocalWorld` then has a direct URL too (`LocalWorld::postgres_direct_over`).

## Tests

* `tests/conformance.rs`: the `AgentClient` conformance suite (`agent_client_conformance!`) against the client,
  over the in-memory journal and over Postgres.
* `tests/history.rs`: a fork's first task (`SendRequest.history`, [ADR 0029](../../../docs/decisions/0029-forking-a-thread-copies-its-log.md)) is told the conversation in front of the message, in the same text, as the A2A client does; a message with none is sent as it is.
* `tests/durability.rs`: a task survives its worker dying mid-step (two processes on one schema, a 1 s lease: the
  second steps the task to its end and a resubscribe shows the original keys); the tables and channels carry the
  prefix and no `adam_` table exists (a session of its own listens on both prefixes' channels); the orchestrator's
  tables (migrated by `orch-store-postgres` on its own pool over the schema's URL), adam's default prefix and ours
  coexist in one schema; the migration is idempotent, alone and under six concurrent replicas.
* `tests/errors.rs` (Postgres): a server's error with a `DETAIL` that quotes a row, kept by `LocalAgentsError`, says
  the message and the SQLSTATE and nothing of the row, in its chain and in its `Debug`; a connection string that does
  not parse, or a database that cannot be reached, is a transient error that holds neither the string nor its
  password; in a child process with `PGPASSWORD` set, a URL with no password is refused as `Invalid` (the notifier's
  listener would not get it), naming the password and never its value, and a URL that names one is taken.
* Unit tests in `src/`: the echo agent and its starter agree, the error class table, and a store's or a worker's
  driver error kept as words with its class.
* End to end through the dispatcher and the binary: [`orch-e2e`](../e2e/README.md) (`tests/local_agent.rs`) and
  [`orchestrator`](../../bin/orchestrator/README.md) (`tests/local.rs`).

## See also

[`orch-ports`](../ports/README.md), [`orch-agent-a2a`](../agent-a2a/README.md),
[`orch-a2a-mapping`](../a2a-mapping/README.md), [`orch-e2e`](../e2e/README.md).
