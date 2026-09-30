# ADR 0016 — Inbox, timers and the job ledger on the thread

- **Status:** accepted (2026-09-30). **Planned, not built:** this ADR is the design for MVP slices 2
  and 5 ([`mvp.md`](../mvp.md#the-slices-of-steps-2-3-and-6)). Refines
  [ADR 0001](0001-rust-state-machine-on-postgres.md) (the transactional inbox it named, and its
  one ledger). Gives the inbox its first user, the CI webhooks of
  [ADR 0017](0017-ci-results-by-webhook.md); the gate that uses the ledger is
  [ADR 0018](0018-verification-gate-and-rework-loop.md).

## Context

MVP step 2 ends a job as a pushed branch plus a CI result, and step 3 gates the job on checks. Both
need things the code does not have (checked against `a35fa57`, 2026-09-30):

- **A place for state that is not the thread's state.** `threads.state` is a text column with a
  `CHECK` and no payload ([data model](../orchestrator.md#data-model)). A gate needs an attempt
  counter, the pushed commit, the results collected so far and the policy in force.
- **Input nobody asked for.** Today every input is a request handler that runs `transition` inside
  `App::apply` and answers the caller, so a redelivery cannot happen on that path. A CI system
  posts a webhook, may post it twice, may post it before the orchestrator knows which thread it
  belongs to, and gives up after a few seconds. [Verified 2026-09-30](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks):
  GitHub expects a 2xx "within 10 seconds of receiving a webhook delivery".
- **Time.** A job waiting on CI must not wait forever, and the core must not read a clock
  ([ADR 0001](0001-rust-state-machine-on-postgres.md), invariant 5 in `CLAUDE.md`).
- **A design that was too broad.** [`orchestrator.md`](../orchestrator.md#event-flow) said "every
  input goes through the inbox", MCP included. An MCP caller is authenticated and waits for the
  answer; parking its request in a table buys nothing ([ADR 0019](0019-mcp-server-over-streamable-http.md)).

The owner decided on 2026-09-30 that CI results arrive by webhook in two shapes (ADR 0017), that
the gate is configurable (ADR 0018) and that MCP comes with bearer tokens first (ADR 0019).

## Decision

### 1. The job lives on the thread

One thread holds one job for MVP steps 2 to 6. The MCP `job_id` is the thread id.

The ledger is a new column, `threads.job jsonb NOT NULL DEFAULT '{}'`. It is written in the same
commit as `state`, under the same `version` compare-and-swap (CAS). Reasons:

- The owner scope, the AG-UI connect stream and the idempotency keys are already per thread.
- [ADR 0001](0001-rust-state-machine-on-postgres.md) wants one ledger, not a second table to keep
  consistent with the first.
- Step 4's child jobs will get a separate job-snapshot table later. That is a later decision; this
  one does not close it.

### 2. The pure core takes and returns a `Snapshot`

`orch-core` stays pure: no async, no I/O, no wildcard arms in any `match` (ADR 0004). Planned
shapes (names and fields are the design; the code may differ in detail):

```rust
pub struct Snapshot { pub state: ThreadState, pub job: Job }

pub fn transition(s: &Snapshot, i: &Input) -> Result<(Snapshot, Vec<Command>), TransitionError>;

pub struct Job {
    gate: GatePolicy, attempt: u32, pushed: Option<PushedRef>,
    results: Vec<CheckResult>, hold: Option<Hold>,
}
pub struct GatePolicy {
    require: BTreeSet<CheckSource>, max_attempts: u32,
    ci: CiPolicy,                       // required names, timeout
    verifier: Option<AgentId>, verifier_timeout: SignedDuration,
}
pub enum CheckSource { Ci, AgentChecks, Verifier }

ThreadState += Verifying
Input       += CiReported(CiReport) | VerifierReported { attempt, verdict } | TimerFired(Timer)
Timer        = CiDeadline { attempt } | VerifierDeadline { attempt }
Command     += Watch { key } | Schedule { after: SignedDuration, timer }
             | RequestVerification { attempt, verifier, pushed, text }
EventBody   += CiResult
             | CheckResult { source, attempt, status: pending|passed|failed, findings }
             | Rework { attempt, max_attempts, findings }
```

- **The gate policy is copied into `Job` when the thread is created.** A configuration change never
  affects a running job.
- **There is no `Reworking` state.** A rework is `Queued` or `Working` with `attempt > 1`.
- **Two artifacts are recognised by a pure function in the core:**
  - `branch {repository, branch, commit}` sets `pushed` and emits `Watch { ci:<repo-key>@<sha> }`;
  - `checks {passed, commit, summary?, findings?}` is the agent-checks result.
- **Repository keys** are normalised to `host/owner/name`, lower-cased, without `.git`, so the key an
  agent's `branch` artifact produces and the key a webhook produces are the same string.

The transition table, the state diagram and the property tests in `orch-core` grow with these; the
existing rows and the goldens do not change when the gate is empty.

### 3. The loop, as rules of the core

These are the rules `transition` implements. The configuration that fills `GatePolicy` and the
projection to the chat are [ADR 0018](0018-verification-gate-and-rework-loop.md).

| Situation | Result |
|---|---|
| The agent reports `completed` and the gate requires nothing | `Done`, exactly as today. |
| The agent reports `completed` and the gate requires something | `Verifying`. One `pending` `check_result` for each source not yet known; `Schedule(CiDeadline)` if CI is required; `RequestVerification` if the verifier is required. |
| Every required source has passed | `Done`. |
| A source has failed and `attempt < max_attempts` | A `rework` event; `Delegate` with the findings text built in the core; state `Queued`; `attempt + 1`. |
| A source has failed on the last attempt | `Failed`, with the findings in `error` and in `check_result` events. |
| CI or the verifier is required and there is no pushed SHA | A failed check: "no pushed commit". |
| A stale timer, or a CI result for an older SHA or attempt | Its event is recorded; nothing else changes. |
| A CI result on a finished thread | Only its card is appended. |
| A user message during `Verifying` | That verification is abandoned and the message is delegated; no attempt is counted. |
| Cancel during `Verifying` | `Cancelled`. |

The timeouts that end in `Blocked` are in [ADR 0017](0017-ci-results-by-webhook.md) (CI) and
[ADR 0018](0018-verification-gate-and-rework-loop.md) (verifier).

### 4. The inbox holds unsolicited machine input, and only that

Its jobs are to answer fast, dedupe redeliveries, and park reports that cannot be matched yet. It
carries webhooks and timers. **MCP does not use it** ([ADR 0019](0019-mcp-server-over-streamable-http.md)),
and neither do AG-UI and the chat API: their idempotency key stays on the event, as today.

Migration `0004` (slice 5) adds two tables:

```
inbox(id, source, idempotency_key, kind, payload jsonb, correlation,
      status pending|inflight|parked|applied|expired|dead,
      available_at, attempts, lease_owner, lease_until, last_error,
      UNIQUE (source, idempotency_key))
watches(key PRIMARY KEY, thread_id)
```

**Timers are inbox rows** with `source = 'timer'` and `available_at = now + after`, so the core
never reads a clock: it asks for a delay with `Schedule` and is handed `TimerFired` when the row is
due.

```mermaid
sequenceDiagram
  participant W as InboxWorker (orch-app, every role that runs workers)
  participant A as App
  participant C as transition (pure)
  participant DB as Postgres
  Note over A,DB: agent reports completed, CI is required
  A->>C: transition(snapshot, Agent completed)
  C-->>A: Verifying, Schedule(CiDeadline{attempt 1}, after 1 h), events
  A->>DB: ONE txn: threads.state + threads.job (version CAS), events, outbox,<br/>INSERT inbox(source timer, key, available_at = now + 1 h)
  Note over W,DB: an hour later, no CI report has arrived
  W->>DB: claim due rows (available_at <= now, SKIP LOCKED), lease
  W->>A: apply(thread, TimerFired(CiDeadline{attempt 1}), key inbox:<id>)
  A->>C: transition(snapshot, TimerFired)
  C-->>A: Blocked (interrupt reason ci_timeout), no attempt spent
  A->>DB: ONE txn: state + job (CAS), events, mark inbox row applied (lease fenced)
```

```mermaid
stateDiagram-v2
  [*] --> Armed: Schedule committed, available_at in the future
  Armed --> Due: available_at reached
  Due --> Inflight: claimed by an InboxWorker (lease)
  Inflight --> Inflight: lease expired, another replica re-claims
  Inflight --> Applied: TimerFired applied, thread changed
  Inflight --> Applied: stale timer (older attempt or finished thread): event recorded, nothing changes
  Inflight --> Dead: permanent error
  Applied --> [*]
  Dead --> [*]
```

What the diagrams cannot say:

- **The write path is one commit.** `Commit` gains `watches`, `timers` and `inbox: Option<Lease>`.
  A `Watch` inserts the `watches` row and, in the same transaction, re-arms parked inbox rows with
  that key ([ADR 0017](0017-ci-results-by-webhook.md)). A `Schedule` inserts its timer row. The
  inbox lease is fenced like an outbox lease ([Fenced commits](../orchestrator.md#fenced-commits)):
  a late worker's commit under a stale lease writes nothing.
- **Flow of a webhook row** (the surface authenticates and normalises first, ADR 0017):
  1. `App::receive` inserts the row and returns 202; a duplicate `(source, idempotency_key)` is also 202.
  2. `InboxWorker` claims rows with `SKIP LOCKED` and resolves `correlation` through `watches`.
  3. It applies `App::apply(thread, Input, key = "inbox:<id>")`.
  4. A row that matches no watch is **parked**. A later commit carrying `Watch { key }` inserts the
     watch and re-arms the parked rows in the same transaction.
  5. Parked rows expire after `INBOX_PARKED_TTL_SECS`.
- **No ordering across rows.** The first completed report for each attempt decides.
- **Ports.** The methods go on the existing `ThreadStore` (`receive`, `claim_inbox`, `park_inbox`,
  `retry_inbox`, `complete_inbox`), so the commit stays atomic; a second port would need a
  distributed transaction. Conformance cases join `thread_store_conformance!`: dedupe, park and
  re-arm in one transaction, fencing, expiry, a timer not yet due.
- **`InboxWorker`** lives in `orch-app` and runs wherever the dispatcher runs: the `worker` and
  `all` roles ([ADR 0015](0015-control-plane-and-workers-on-adam-rs.md)). A control plane only
  writes rows.

### 5. Machine routes

`SurfaceRoutes::machine(router, guard)` in `orch-api` mounts routes **outside** the oauth2-proxy
identity layer. The `guard` is a required authenticator (an HMAC check, a bearer check), so a
machine route cannot be mounted without one. **Machine routes never read `X-Auth-Request-Email`.**
Today `router_with_surfaces` wraps every route in the identity layer
([Design choices](../orchestrator.md#design-choices)); the machine mount is the one exception, and
it is explicit.

### 6. Secrets

- Stored as `secrecy::SecretString`.
- HMAC is checked with `hmac`'s `verify_slice`; tokens are compared with `subtle` over SHA-256
  digests of both sides.
- `Config`'s `Debug` stays redacted, and a test says so.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `INBOX_PARKED_TTL_SECS` | `86400` | How long a report that matches no watch waits before it expires. |
| other `INBOX_*` (lease, poll interval) | set in slice 5 | The plan names only the parked TTL; slice 5 documents the rest beside the code. |

## Security notes

- Machine routes are the only routes without the identity layer. Each one is fail closed: the guard
  runs on the raw body before any write ([ADR 0017](0017-ci-results-by-webhook.md)).
- The inbox stores payloads that came from outside. A payload is data: CI summaries flow into the
  rework prompt only as quoted, untrusted text ([ADR 0018](0018-verification-gate-and-rework-loop.md)).
- A webhook cannot approve or merge anything. A `CiReported` input can add a check result and
  nothing else; `transition` decides by input, not by who is connected.
- Secrets never appear in logs, `Debug` output or the inbox `payload`.

## Consequences

**Easier**

- Redelivered webhooks are harmless; a report that arrives before its watch is not lost.
- Time is data: tests advance an injected clock, replay stays deterministic.
- A worker crash mid-inbox costs latency, not correctness (the lease lapses, another replica takes
  the row).
- The chat and MCP still see one thing: the thread and its event log.

**Harder**

- `transition`'s signature changes from `(&ThreadState, &Input)` to `(&Snapshot, &Input)`. Every
  table test and property test in `orch-core` is touched. This is why slice 2 is the large one.
- `threads.job` is a `jsonb` blob owned by the core's serde shapes: a change to `Job` is a
  compatibility question for rows already written.
- One thread, one job. Step 4's parallel child jobs need another table and another decision.
- Migrations `0003` (slice 2) and `0004` (slice 5) are fixed now; parallel slices must not claim
  those numbers.

## Alternatives considered

- **A separate `jobs` table now.** Rejected: a second row to commit in step with the thread, and the
  owner scope, connect stream and idempotency keys would all have to be re-derived from the job.
  Step 4 will need a job-snapshot table for child jobs; that is the moment to add it.
- **Every input through the inbox, MCP too** (the earlier design in `orchestrator.md`). Rejected: the
  MCP caller is authenticated and waits; the inbox would only add a hop and a place for the answer
  to get lost ([ADR 0019](0019-mcp-server-over-streamable-http.md)).
- **Apply webhooks synchronously in the request.** Rejected: the sender's deadline is 10 s and a
  slow commit or a CAS conflict would turn into a failed delivery; and a report that beats its
  watch would have nowhere to wait.
- **A clock in the core** (`now()` inside `transition`). Rejected: it breaks purity and replay.
  Timers as inbox rows keep the core a function of its inputs.
- **A dedicated timer table.** Rejected: an inbox row already has availability, a lease, retries and
  a status; a second mechanism would repeat them.
- **A second port for the inbox** (`InboxStore`). Rejected: the write of the inbox row's completion
  must be in the thread's commit. Two ports would mean two transactions.

## Hard to reverse

- **`transition`'s signature and `Snapshot`.** Every caller and test depends on them.
- **The `threads.job` column and the serde shape of `Job`.** Rows written under it must be read
  forever, or migrated.
- **The `inbox` `UNIQUE (source, idempotency_key)`** and the idempotency key formats
  (`github:<delivery>`, an external contract once senders rely on them; `inbox:<id>` for applied rows).

Easy to reverse: the parked TTL, the defaults, and the choice to put the worker in every role.

## Verified

- *Verified 2026-09-30* (this repository at `a35fa57`): migrations `0001_init.sql` and
  `0002_ui_events.sql` exist and `threads.state` and `events.kind` are text columns with `CHECK`s;
  there is no `inbox`, `watches` or timer table; the outbox kinds are `delegate` and `cancel`.
  Widening a `CHECK` is a whole migration (`0002` did it for the UI events).
- *Verified 2026-09-30*: GitHub expects a 2xx response within 10 seconds of receiving a delivery.
  Source: <https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks>.
- *Unverified*, checked in the slice that uses each: that `hmac`'s `verify_slice` compares in
  constant time; the current version of `secrecy`; the behaviour of `SKIP LOCKED` claims under the
  `available_at` ordering at the volumes of a real deployment.
- *Unresolved detail for slice 2:* a user message that abandons a verification does not count an
  attempt, so the next verification can carry the same `attempt` number as the abandoned one. A
  `CiDeadline { attempt }` left over from the abandoned verification would then look current. Slice
  2 must tell the two apart (for example with a verification counter in `Job`) and add a
  transition-table row and a property test for it.
