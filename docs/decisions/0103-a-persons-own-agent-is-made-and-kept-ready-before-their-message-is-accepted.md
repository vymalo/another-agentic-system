# ADR 0103 — A person's own agent is made and kept ready before their message is accepted

- **Status:** **Proposed (2026-11-26)** (ANR-806, part 2; built in `orchestrator/crates/app/src/provisioning.rs`, off unless the deployment has a `provisioner` section). Not yet tried against a real platform: the dev stack plays it with a WireMock stand-in (`dev/provisioner-e2e.sh`). **Amended (2026-11-27): ANR-1061, a person's message goes to their own agent by a route recorded in `Job.route`, and the dispatcher's sends for a routed thread provision too (decision 5, the amendment below).**

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
  agent only. A stop whose agent is not listed is decided by `cancel`, which never wakes the agent: with no task running at it the thread is cancelled locally
  (`CancelledBeforeStart`) at once; a delegation in flight is waited for as for any thread; and with a task running at the
  unlisted agent the stop is tried for up to `own_agent_listing_attempts` attempts of the row in all and then ends the thread
  locally with a warning that the task at the agent could not be stopped.
- **The tier and the cap come from the own agent's card.** The request checks a tier against the thread's agent, the send against
  the card of the agent the row goes to (`tier_for_send`), so an own agent that does not list `access-tier/v1` gets no tier, and one
  that lists it is given the message under it. The same holds for the model pick and the other extensions read from the live card.
- **Unchanged:** the sends that name an agent other than the thread's (`ask_agent`, `@mention`) and ask the platform nothing.

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
  - **A rollback past this change is one-way.** An older binary drops `route` from `job_json` on its next write, and the next
    message then goes to the shared agent carrying the person's agent's context. Do not roll back past it while provisioning is
    on.
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
