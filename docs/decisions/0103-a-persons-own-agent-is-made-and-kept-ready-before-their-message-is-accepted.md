---
sidebarTitle: "0103 · Own agents kept ready"
---

# ADR 0103 — A person's own agent is made and kept ready before their message is accepted

- **Status:** **Proposed (2026-11-26)** (ANR-806, part 2; built in `orchestrator/crates/app/src/provisioning.rs`, off unless the deployment has a `provisioner` section). Not yet tried against a real platform: the dev stack plays it with a WireMock stand-in (`dev/provisioner-e2e.sh`). **Amended (2026-11-27): ANR-1061, a person's message goes to their own agent by a route recorded in `Job.route`, and the dispatcher's sends for a routed thread provision too (decision 5, the amendment below).** **Amended (2026-11-27): ANR-1078, the two known gaps of a stop that waited are fixed by refunding the wait (note under Consequences).** **Amended (2026-11-27): ANR-1078 PR 2, a routed row waiting for the person's agent to wake is cut at a short probe and waits `pending`, so it no longer holds a dispatcher slot, and a stop that finds it between two probes ends the thread before it starts (the amendment `Amended 2026-11-27 (ANR-1078, PR 2)` below).**

## Context

The platform can make an agent for a person (`AgentProvisioner`, `orch-provisioner-platform`). Nothing asked it to: a person's
first message to the coder went to the deployment's one configured agent, and a suspended agent stayed suspended. The message
path needs three answers it did not have: which person a platform agent belongs to, which messages are for which kind of agent,
and how long a person may be made to wait.

## Decision

1. **The owner key is a pseudonym of the user id.** It is the first 24 hex digits (96 bits) of the SHA-256 of the user id,
   **unsalted** (`Provisioning::owner_of`). It is stable (the same person always has the same agent), distinct for two people,
   short enough for `workspace-<owner>` to stay under the platform's 40 characters, and it names nobody: an e-mail is not a legal
   key. It is **not a secret**: anyone who knows a user id can compute it, and the platform's own access rules, not the key,
   keep one person out of another's agent. It is a function of the id the identity provider gives, so **a deployment that
   changes the claim that names a person (`sub`, an e-mail) moves every person to a new key and so to a new agent**; the old
   ones are left where they are. That change is a migration, not a setting.
2. **The agent's names are the switch.** The kind is read from the agent's id **and its aliases** (ADR 0049): an agent whose id
   or alias is `coder` is the coder kind, one called `workspace` is the workspace kind, and only kinds the deployment lists in
   `provisioner.kinds` are provisioned. A thread is created under the agent's canonical id, so the alias is how the dev stack's
   `adam` (alias `coder`) is found. **Aliasing any agent to `coder` or `workspace` turns provisioning on for it**; nothing else
   does, and the shared `chat` agent never is.
3. **The message waits, within a bound the person is told.** `ensure_ready(owner, kind)` runs before what starts the agent's
   work is written: a first message, a message or Stop & send, an action on a card, a message that opens a fork, and the start
   of a helper session. It makes the agent, resumes a suspended one, and waits until it is `Ready`. The bound is the **wait-ready
   bound**, `provisioner.readyTimeoutSecs` (default 120 s), plus a **10 s backstop** (`OUTER_SLACK`) for an adapter that does not
   bound itself. A wait that ends in `ProvisionError::NotReady`, or at the backstop, is `AppError::AgentNotReady`, class
   `Transient`: **HTTP 503**, code `agent_not_ready`, `Retry-After: 5`, naming the bound and the last state, with **nothing
   written**, so the caller sends the same message again. Any other refusal of the platform is `AppError::Provisioning`, and a
   platform that refuses the orchestrator or its form is a 500 (the deployment's fault, not the person's).
4. **A replay is not a reason to ask.** A request whose idempotency key is already in the log (`ThreadStore::key_recorded`), a
   first message whose thread already exists, and a fork that is the replay of an earlier one write nothing, so they do not ask
   the platform and get the acceptance they had even while the platform is down.
5. **What this does not do.** ~~The message still goes to the deployment's **configured agent**; routing it to the person's own
   agent is a follow-up. Sends that do not come from a person's request (the verifier and CI re-delegation, a redelivery, an
   `@mention` or `ask_agent` to a coder) do not provision: the dispatcher does not ask the platform.~~ *Superseded
   2026-11-27 by the amendment below (ANR-1061): the message goes to the person's own agent, and the dispatcher's sends for a
   routed thread provision too. What stays true: an `@mention` or `ask_agent` to a coder does not provision.*

## Amended 2026-11-26 (ANR-1061): the message goes to the person's own agent

Decision 5 left the message at the deployment's configured agent, so the agent this ADR makes was never used. It is used now.
This amendment replaces its first version, which recomputed the route from the configuration on every dispatch.

- **The route is recorded, not recomputed.** When a thread's first message provisions the person's agent (`create_thread_as`, or
  a fork), the thread's job ledger records the agent: `Job.route`, the `ProvisionedAgent.name` (`<kind>-<owner>`). It is an
  additive, optional field of the ledger (no migration; a ledger without it reads as none), kept by every later job like the
  title or the attached servers. The thread's **target stays the configured agent** (`coder`, `adam`): the gates, the tiers, the
  tool grants and the permissions are keyed by it and do not change.
- **`Dispatcher::load` uses the recorded route for every row kind** (delegation, action, steer, cancel, follow, helper-session
  start and close, redelivery), never the configuration. A row for a thread with no route goes where it always went. **A helper
  session's thread inherits its parent's route when it is opened** (`open_child` copies it, the core stays pure), so the helper's
  follow, a message the person sends in it and its close go to the same agent, in the context that agent made.
- **A thread that began before provisioning keeps its agent and its context.** Its job has no route, so no mid-thread switch:
  the shared agent holds its A2A context and task, and a message to it asks the platform nothing and makes no agent nobody uses.
  (Whether an agent refuses a context it did not make is *unverified*; the design does not rely on it.)
- **A fork has a binding of its own and no context at either agent yet**, so it records its owner's own agent when the agent's
  kind is provisioned, which for a person forking their own thread is the parent's.
- **Which sends provision.** The person's requests (a message, Stop & send, an action, a fork's message, a helper session's
  start) call `ensure_ready` for a routed thread, as in decision 3. Every row the dispatcher claims for a routed thread does too
  before it is loaded, so a re-delegation by the verifier or CI, a redelivery and a session start find a suspended agent resumed;
  one platform read per row when the agent is up. **A stop (`Cancel`) and a follow (`Observe`) never wake an agent.**
- **Not listed yet.** The platform says `Ready` before its registry may list the agent. A row for a routed thread whose agent the
  registry does not list is tried for up to `own_agent_listing_attempts` attempts of the row in all (`DispatcherConfig`, **default 5**, about 15 s
  with the default backoff; not that many extra retries), with a debug log each time and a warning when it is given up, and then fails with its own reason: "your agent
  ... is ready but is not listed yet" when this row's own provisioning call returned the agent ready, and, when no call was made
  or provisioning no longer covers the kind, "your agent ... is not listed and provisioning is not on for it", and for a follow,
  which never wakes the agent, "your agent ... is not listed (following a task does not wake it)". A row on a thread that has
  ended does not wake the agent either. It is never sent to the shared agent in its place, and the retry applies to the thread's own
  agent only. A stop whose agent is not listed is decided by `cancel`, which never wakes the agent: with no task running at it and nothing in flight the thread
  is cancelled locally (`CancelledBeforeStart`) at once; a delegation in flight is waited for, and the stop then stops the task it
  started (for a routed thread the wait's budget is `max_cancel_attempts` plus the retries that fit in `readyTimeoutSecs` plus
  `OUTER_SLACK`, so a stop pressed while the agent wakes is not lost); and with a task running at the
  unlisted agent the stop is tried for up to `own_agent_listing_attempts` attempts of the row in all and then ends the thread
  locally with a warning that the task at the agent could not be stopped.
- **The tier and the cap come from the own agent's card.** The request checks a tier against the thread's agent, the send against
  the card of the agent the row goes to (`tier_for_send`), so an own agent that does not list `access-tier/v1` gets no tier, and one
  that lists it is given the message under it. The same holds for the model pick and the other extensions read from the live card.
- **Unchanged:** the sends that name an agent other than the thread's (`ask_agent`, `@mention`) and ask the platform nothing.

## Amended 2026-11-27 (ANR-1078, PR 2): a row waits for the agent to wake without holding a slot

The text of the 2026-11-26 amendment above is kept as written. Two sentences of it changed: a routed row **held a dispatcher
slot** for as long as the agent took to wake (the wait-ready bound plus `OUTER_SLACK`, 130 s by default), so a few people's
slow agents could take every slot (`concurrency`, 32 by default) from everyone else; and a stop pressed while the agent woke
**waited for the delegation in flight** and then cancelled at the agent ("the stop waits, then cancels at the agent", the
*Not listed yet* bullet).

- **The wake is a probe.** `Dispatcher::load` waits for the person's agent to answer that it is ready for
  `DispatcherConfig.own_agent_probe` (2 s) and no longer: `tokio::time::timeout` cuts the call. The cut drops the **wait**, never the request: the wake runs
  in a task of `Provisioning`, one per (owner, kind), joined by every caller (the dispatcher's rows, so a person's N waiting rows
  make **one** request to the platform, and the request path), and a result, an error included, is delivered to each waiter.
  The entry leaves the map when the task ends, which the backstop (`readyTimeoutSecs` plus `OUTER_SLACK`) bounds; the map is
  process-local and holds no datum of the person's, so invariant 3 stands. Without it a platform slower than the probe (one
  request may take 5 s, with a cold token fetch more) would have every probe drop the request and the row die on an agent that
  would have come up. Whether the platform finishes a create or a resume whose client vanished is *unverified*; the flight makes
  it not matter for the dispatcher. The next claim finds the agent, the port's rule (below) covering a restart. A probe that runs out, or a platform that answers
  `AgentNotReady`, sends the row back with `ThreadStore::wait_outbox` (the refund of PR 1, Consequences) after the
  **fixed** delay `own_agent_wait_delay` (2 s), not the exponential backoff: with the backoff a row could sit up to 60 s after
  the agent was ready. The claim is refunded, so `max_attempts` and the other caps read only the claims that tried something. A
  platform error other than not-ready (`AppError::Provisioning`) keeps the counted retry. **A behaviour change:** an `AgentNotReady`
  was a counted retry with the backoff under `max_attempts`, so an agent that stayed `Blocked` or `Degraded` was waited for
  about 5 times 130 s, near 11 minutes; now the row waits about 130 to 200 s in all and is dead, so a degraded rollout longer
  than that loses the message (the thread is told, and the person sends it again). `AppError::AgentNotReady`, whose
  `waited` names the whole bound, is **not** reused for the dispatcher's wait.
- **The row gives up when the wake bound is spent.** A row whose refunded claims reach the cycles (a probe and its wait) that fit
  in `own_agent_wake_bound()` (`readyTimeoutSecs` plus `OUTER_SLACK`) ends as the row for an agent that cannot be made ready did
  ("your agent could not be made ready", dead `own agent not ready`). **The bound is a count of cycles, not a duration:** a cycle
  is the probe, the delay and the claim's latency (up to `poll_interval`), so the 33 cycles of the defaults take about 130 to
  200 s, and a provisioner that answers not-ready at once has shorter cycles, so it gives up sooner. Every wait is thus bounded by that count: no row waits for
  ever or spins. The cost is one platform read per cycle per waiting row: a person's N waiting rows read the platform every
  `own_agent_probe` plus `own_agent_wait_delay`.
- **The port's rule.** `AgentProvisioner::ensure_ready` is cut at any `.await`, so *a dropped call leaves the agent coming up
  and the next call finds it* (conformance case `a_dropped_call_leaves_the_agent_coming_up_and_the_next_call_finds_it`). The
  Platform API adapter already held it (it polls a state the platform drives, and `create` and `replace` are single requests,
  idempotent by `If-None-Match` and `If-Match`). The in-memory provisioner did not: the caller's sleep turned `Saving` into
  `Ready`, so its readiness is a stored instant now.
- **A stop between two probes cancels before the start.** A delegation that waits for its agent is `pending`, unsent, between two
  probes, and `skip_unsent_delegates` skips it: the stop ends the thread `CancelledBeforeStart` at its first claim and nothing is
  sent. Before this amendment the delegation was in flight for the whole wake and the stop waited for it. **Accepted.** The stop's
  wait for a delegation in flight (decision of 2026-11-26 above, with its wake headroom and the refund of PR 1) is still reached
  by a delegation a claim holds: at its probe (at most `own_agent_probe`), and at the send that follows a wake. A stop that
  races a probe waits for it as before. A **Stop & send** between two probes is the same stop, and its follow-up job (`after_stop`,
  ADR 0036) is a new delegation that wakes the agent again; only the plain stop is tested.
- **The tunables are two fields of `DispatcherConfig` and no configuration key**, so `docs/config.md` is untouched and the
  composition root keeps the defaults. They are public fields: **a struct literal of `DispatcherConfig` outside the workspace
  breaks** (use `..DispatcherConfig::default()`).
- **Known limit.** The claim order is by row position, within a person's queue (ADR 0066), so a person with at least twice as many
  waiting routed rows as slots can keep their own *unrelated* newer rows from a slot, as they could before; another person's rows
  are not held back, and no row holds a slot for more than a probe.

## Consequences

- A deployment without a `provisioner` section is exactly as it was: `AppConfig.provisioning` is `None`.
- A person's first message to the coder can take up to the wait-ready bound. A bound that is too short makes first messages
  fail with the 503; one that is too long holds a request open.
- `ThreadStore` gains a provided method, `key_recorded` (default: no), which the in-memory, SQLite and Postgres stores override
  and the conformance suite holds; an implementer that has not written it keeps compiling and treats every request as a first
  one (the commit still refuses the duplicate).
- The 503 is additive to the chat API (`docs/api/chat-api.yaml`).
- Changing the identity claim is a data migration of platform agents (decision 1).
- **ANR-1061 (amendment of 2026-11-27):**
  - The job ledger gains the optional field `Job.route` (the person's agent's id, absent for a thread that began before
    provisioning); `DispatcherConfig` gains the public field `own_agent_listing_attempts`, which **breaks any struct literal of
    it outside the workspace** (use `..DispatcherConfig::default()`).
  - A routed row holds a dispatcher slot while it waits for the agent, up to `readyTimeoutSecs` plus slack per row; a
    follow-up card will bound it.
    - *Note 2026-11-27 (ANR-1078, PR 2): bounded.* A routed row waits for the agent without a slot: the wake is cut at
      `own_agent_probe` and the row waits `pending` for `own_agent_wait_delay` with its claim refunded, until the wake bound
      is spent (the amendment `Amended 2026-11-27 (ANR-1078, PR 2)`); a slot is held for a probe at most. The sentence above is
      kept as it was written.
  - **A rollback past this change is one-way.** An older binary drops `route` from `job_json` on its next write, and the next
    message then goes to the shared agent carrying the person's agent's context. Do not roll back past it while provisioning is
    on.
  - **Known gaps (follow-up ANR-1078).** A stop has one attempt counter for its three phases (the wait for a delegation in
    flight, the listing wait, `CancelTask`), and only the first has the wake headroom; no stored datum (the binding keeps no
    time, a finished delegation's row is not reachable from the stop) says how many attempts a stop spent waiting. So (A) a
    transient `CancelTask` error right after a long wake is not retried when the wait used up `max_cancel_attempts`, and the
    stop is rejected; and (B) an agent that is not yet listed right after the wake is cancelled locally, without the listing
    attempts, when the wait used up `own_agent_listing_attempts`. A stop that never waited has exactly the plain caps.
    - *Note 2026-11-27 (ANR-1078, PR 1): gaps A and B are fixed by a refund, and the text above is kept as it was written.*
      The outbox row now says how many of its claims only waited: `outbox.refunded` (`OutboxItem::refunded`, the inbox's
      `refunded` of ADR 0016 over again), raised by the new `ThreadStore::wait_outbox`, which the stop's wait for a
      delegation in flight calls instead of `retry_outbox`. `attempts` still only goes up (it is the fencing token of the
      lease); every cap and backoff of the dispatcher reads `OutboxItem::counted_attempts()` (`attempts - refunded`), so
      the listing wait and the `CancelTask` of a stop count only the claims that tried something (they share that one counted
      counter, and a lapse counts). The checks that ask whether an earlier
      claim may have sent the message (the lookup by message id of a delegation, a verify and an ask) stay on the raw
      `attempts`, and so does the bound of the wait itself, the one cap that has to count the waits (so `wake_headroom`
      is still needed). A stop that never waited is as it was. A rolling deploy: a replica of an older build that is
      already running ignores `refunded` and may dead-letter a waited row earlier than this build would (the row ends as it ended
      before); a restarted old-build pod refuses to start, its own `orch-pg-migrate` returning `VersionMissing` as for every migration. Not
      changed: a claim handed back at shutdown (`release_leases`) is still counted for an outbox row. *Unverified until CI has run it:*
      `crates/app/tests/provisioning.rs` and the store conformance case
      `a_wait_is_refunded_and_a_retry_and_a_lapse_are_counted` (memory, Postgres and SQLite); nothing was compiled or run on the host that wrote it.
  - The dispatcher re-reads the thread and the binding after a row's wake (`ensure_ready`), so it never acts on state from before the wait.
  - Turning provisioning off, or removing a kind from `provisioner.kinds`, **strands the routed threads**: they never return to
    the shared agent (a row for one that is not listed ends with the reason above).
  - The request-time checks of tier, model pick, uploads and child sessions read the **shared agent's** card and are
    pre-checks only; the send is checked against the person's own agent's card.

## Alternatives rejected

- **A salted or keyed owner hash.** It would hide the pseudonym from someone who knows a user id, but the key is no secret to
  begin with, and a key that depends on a secret moves every person's agent when the secret rotates.
- **A kind named in the agent's configuration.** More explicit than the alias, but a second place to say what the alias already
  says in the dev stack, and the deployment would have to keep the two equal.
- **Provisioning from the dispatcher.** It would cover every send, but the person would see a message accepted that then waits on
  the platform with nowhere to say so; at the request the answer can be a 503 that writes nothing.
  *Note 2026-11-27 (ANR-1061): partly adopted for routed rows.* The dispatcher's own sends for a routed thread (re-delegation,
  redelivery, a steer, a session start) now wake the person's agent, because the message path alone cannot cover them and the
  agent they go to must be up. The 503-at-request argument still holds for the message path: a person's message still waits
  and is refused with nothing written at the request, never accepted and left waiting in the dispatcher.
