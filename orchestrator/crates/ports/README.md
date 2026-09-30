# orch-ports

One trait per infrastructure boundary of the orchestrator, plus a conformance
testkit and in-memory implementations.

## Where it sits

The **ports** of the ports-and-adapters split
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md),
[`docs/orchestrator.md`](../../../docs/orchestrator.md)). `orch-app` and
`orch-api` are written against these traits; the adapters are separate crates:
[`orch-store-postgres`](../store-postgres/README.md) (`ThreadStore`, `Wakeup`)
[`orch-agent-a2a`](../agent-a2a/README.md) and [`orch-agent-adam`](../agent-adam/README.md) (`AgentClient`). No
implementation type appears in a signature, and swapping happens at build time
in the composition root, not through runtime plugins. Depends on
[`orch-core`](../core/README.md) only.

## API at a glance

| Trait | What |
|---|---|
| `ThreadStore` | threads (with their job ledger: `Commit.job` is written in the same commit as the state, under the same version check, and `None` leaves it; a thread created without one has the default job, no gate), the per-thread event log with a strictly increasing `seq` (`commit` is atomic and version-checked; `list_events` reads it forward from a cursor, `latest_events(thread, kind, limit)` the newest events of one kind, newest first, in one bounded read, which `get_job` uses to find the pull request instead of scanning the log), the A2A binding, and the outbox (`claim_outbox`, then `renew_lease`, `mark_sent`, `retry_outbox` and `complete_outbox`, each taking the claim's `Lease { id, owner, attempt }`, as does `Commit.lease`: the fencing token, so a worker whose row was claimed again is refused with `false` / `CommitOutcome::Fenced`; `OutboxItem::lease()` builds it from a claimed row; `mark_verify_sent(lease, task_id, now)` (for a `verify` row: records that the request reached the verifier and its task, on the row and never on the binding, fenced by the claim; a `verify` row is claimed without the per-thread ordering delegations have), `skip_unsent_delegates`, `release_leases`, `get_outbox`, `list_open_outbox`, and `outbox_stats(now) -> OutboxStats { due, waiting, leased, oldest_due_at }`, the counts behind `/metrics`); `ping` for readiness; and the **inbox** (ADR 0016), on the same trait so a commit is one transaction across the thread and the inbox row it applies: `receive(NewInbox { id, source, idempotency_key, payload: InboxPayload, correlation }, now) -> Received::{Stored { id }, Duplicate}` (the same `(source, idempotency_key)` is one row), `claim_inbox(owner, now, lease, limit)` (`pending` and `available_at <= now`, or `inflight` with a lapsed lease; earliest first; a timer not yet due is not claimed), `park_inbox(&InboxLease, now) -> Parking::{Parked, Rearmed, Lost}`, `retry_inbox`, `complete_inbox(lease, InboxFinal::{Applied, Dead { error }}, now)`, `expire_parked_inbox(parked_at_or_before, now)`, `release_inbox_leases(owner, now)`, `get_watch(key)`, and the inspection pair `get_inbox` / `find_inbox`. An `InboxLease { id, owner, attempt }` is fenced like the outbox `Lease` and is a type of its own. `Commit.finishes_outbox: Option<OutboxFinal>` finishes the claimed row (`Commit.lease`) in the same transaction as what the commit writes, so a worker that dies right after the commit cannot have the row redone (the redelivery of a message after a finished job is applied that way; a stale claim is `Fenced` and writes nothing). `OutboxPayload::Delegate { text, release, new_job }` says with `new_job` that the message started a job (its commit appended `job_started`), so the dispatcher opens a new A2A task for it instead of continuing the last one (omitted from the JSON when `false`, read as `false` when absent). `Commit` carries `watches: Vec<WatchKey>` (inserted in the commit, which also re-arms the `parked` rows whose correlation matches, `pending` and due at the commit's `now`; the first thread to watch a key keeps it), `timers: Vec<NewTimer { id, after, timer }>` (an inbox row with `source = "timer"`, due `after` the commit's `now`, key `<thread>:<kind>:<attempt>:<verification>` so a replay arms nothing twice) and `inbox: Option<InboxLease>` (the row is `applied` in the same transaction; a stale claim is `Fenced`, before the version is looked at). `InboxItem` hands the payload as JSON and `decode()` reads it, so a row this build cannot read never spoils the batch it was claimed with. `attempts` counts claims and only goes up (it is the fencing token); `refunded` counts the claims that do not use up the attempt limit (one released at shutdown by `release_inbox_leases`, and every claim up to the one that ended in `park_inbox`, or in the re-arm of a parked row), and `counted_attempts()` is what is left, which `INBOX_MAX_ATTEMPTS` limits. A commit with an `inbox` claim and nothing else (`Commit::only_finishes_inbox`) and the thread's current state is how an input that changes nothing finishes its row: the claim and `expected_version` are checked, the row is `applied`, and the thread is left as it was (no version bump, no `updated_at`, no event) |
| `Wakeup` | `notify(Topic)`, `subscribe()`, `capabilities()`; `Topic` is `Thread(ThreadId)`, `Outbox`, `Inbox` (a report was received or a parked one re-armed; a timer coming due is not announced) or `Resync` (a hint only: the store is the truth) |
| `AgentClient` | `read_card` (`AgentCardInfo { description, version, releases, ui }`: `ui` is `UiSupport { versions }`, present only when the live card lists the A2UI extension), `send_stream` (a `SendRequest` carries `content: SendContent::{Text, UiAction { action, at }}` and `reference_task_ids`, the earlier tasks a new task of the thread is about, A2A `referenceTaskIds`: the previous task for a rework, a follow-up after the turn ended and the first task of a new job, none for a thread's first task, for a continued task and for the verifier; [ADR 0021](../../../docs/decisions/0021-context-across-a2a-tasks.md)), `resubscribe`, `get_task`, `cancel`, `find_task_by_message`; an agent is an `AgentEndpoint { id, transport }` where `AgentTransport` is a closed enum (`A2a { card_url, bearer }`, whose `Debug` redacts the bearer, and `Local { name }`, an agent hosted in the orchestrator's own process; build one with `AgentEndpoint::a2a` or `AgentEndpoint::local`; both variants are always compiled) |
| `ByTransport<A, L>` | an `AgentClient` made of two: `a2a` serves `AgentTransport::A2a` endpoints, `local` serves `AgentTransport::Local`, decided by one exhaustive match in every method, so a new transport must be given a client before the workspace compiles. It holds no adapter type (`A` and `L` are any two `AgentClient`s); the binary builds it in a build with local agents |
| `Clock`, `IdGen` | time and identifiers; `SystemClock`, `UuidV7Ids` |
| `Ports`, `PortSet` | static-dispatch bundle of all five, chosen at build time |

Errors are `StoreError`, `WakeupError` and `AgentError`; each implements
`orch_core::Classify`, so callers decide retry and status by `ErrorClass`, not
by variant. The store and agent traits use `impl Future` methods (no
`async_trait`).

```rust
use orch_ports::{PortSet, Ports, SystemClock, UuidV7Ids};

// Composition happens at build time: pick one implementation per port.
let ports = PortSet { store, wakeup, agents, clock: SystemClock, ids: UuidV7Ids };
let _store = ports.store();
```

## Features

| Feature | Default | Effect |
|---|---|---|
| `testkit` | no | `memory::{MemoryStore, MemoryWakeup, ScriptedAgent, FixedClock, SeqIds, ..}`, and the conformance `testkit` with the macros `thread_store_conformance!`, `wakeup_conformance!` and `agent_client_conformance!`. `ScriptedAgent` runs the scripts `echo`, `ask`, `ui` (an A2UI surface in one payload, then `input-required`; the follow-up, an action or text, finishes the task; `set_ui(agent, versions)` changes what the card lists), `gate`, `slow`, `failed` (fails the task with a message; `fail` rejects the send) and `drop`, and `set_unreachable(agent)` makes every call to that agent fail as if nothing listened. Enable it as a **dev-dependency** feature in adapter crates |

## Tests

* `tests/memory_conformance.rs`: the testkit against the in-memory
  implementations (always runs). The in-memory store is the reference
  implementation of the suite.
* The store cases `stale_attempt_is_fenced`, `commit_after_another_owner_reclaims_is_fenced`,
  `commit_after_complete_is_fenced` and `expired_unclaimed_lease_still_commits` pin the fence.
* The store case `event_data_roundtrip` also pins the A2UI kinds and the `OutboxPayload::Action` row (a `delegate` row whose payload is an action).
* `OutboxPayload::Cancel { job }` names the job of the thread the person asked to stop (ADR 0020); `None` (a row written before the field) means the current job. The store cases `gate_events_roundtrip` (now with `job_started`) and `job_roundtrip` (a ledger with `number: 3`) pin the new event and ledger field.
* The store cases `job_roundtrip`, `job_is_written_with_the_state` and `gate_events_roundtrip` pin the job ledger: it comes back exactly from every read, a commit without a job leaves it, a refused commit (version conflict, replayed key, fenced lease) writes no job, one of several racing writers wins with its job, and the gate's events and the `verifying` state are stored.
* The store case `verify_rows_are_unordered_and_keep_their_task_on_the_row` pins the `verify` outbox kind (ADR 0018, slice 10): the payload and `OutboxItem::task_id` come back, a verify row is claimable while the delegate of its thread is in flight and does not hold the next delegate back, `mark_verify_sent` is fenced (another owner, a stale attempt) and leaves the binding alone, and a re-claimed row still has its task.
* The store cases `inbox_dedupes_by_source_and_key`, `inbox_claims_are_leases_and_lapse`, `inbox_claimers_never_share_a_row`, `inbox_parks_and_rearms_in_one_commit`, `inbox_park_finds_a_watch_that_appeared`, `inbox_commit_is_fenced_and_marks_applied`, `inbox_stale_lease_writes_nothing`, `parked_rows_expire`, `timer_is_claimed_only_when_due`, `a_replayed_commit_arms_no_second_timer`, `inbox_retry_complete_and_release`, `create_thread_arms_timers_and_watches`, `watches_are_first_come`, `inbox_counts_only_the_claims_that_failed` and `inbox_only_commit_finishes_the_row_and_leaves_the_thread_alone` pin the inbox: a redelivery is a duplicate in whatever status the row has become, a lapsed claim is taken over and the late owner's every write is refused, a refused commit (wrong version, replayed key) neither adds a watch nor re-arms a row, a commit under a stale inbox claim writes nothing at all, a timer is claimed from its due time on and no earlier, a claim released at shutdown or ended by a park is refunded while a lapse and a retry stay counted (and `attempts` never goes down), and a commit that carries only its inbox claim checks the version, finishes the row and leaves the thread alone. `wakeup_delivers_inbox_topic` pins the topic.
* `tests/agent_conformance.rs`: the `AgentClient` testkit against `ScriptedAgent`. `ScriptedAgent::set_verifier(agent, VerdictScript)` makes an agent play the verifier (whatever it is sent, it answers a `verdict` that passes or has findings, a garbled one, none, nothing until cancelled, after a gate, a verdict on the task before the gate, a failed task or a question): the dispatcher's verifier tests use it. `fail_next_finds(n)` makes the next lookups by message id fail, and `MemoryStore::fenced_commits()` counts the commits refused for a lost claim, which a test waits for instead of sleeping.
* Unit tests in `src/` pin the error classification tables and `ByTransport` (`routes_by_transport_and_never_crosses`: two scripted agents, every operation once per transport, neither ever sees the other's endpoint).

Adapters run the same testkit; see
[`orch-store-postgres`](../store-postgres/README.md). To add a `ThreadStore`
or `Wakeup` implementation:

```rust
async fn make() -> Option<MyStore> { /* fresh, isolated store; None skips */ }
orch_ports::thread_store_conformance!(make);
```

An `AgentClient` implementation supplies an `AgentFixture` (the client, an endpoint of an agent that runs the scripts `echo`, `ask`, `gate`, `slow` and `fail`, an endpoint nobody listens on, and the gate the `gate` script waits for; override `text(script)` if the agent's words differ):

```rust
async fn make() -> Option<MyFixture> { /* client + a healthy agent; None skips */ }
orch_ports::agent_client_conformance!(make);
```

The calling crate needs `tokio` (with `macros` and `rt`) as a dev-dependency.

## See also

[`orch-core`](../core/README.md), [`orch-app`](../app/README.md),
[`orch-store-postgres`](../store-postgres/README.md),
[`orch-agent-a2a`](../agent-a2a/README.md).
