# ADR 0103 — A person's own agent is made and kept ready before their message is accepted

- **Status:** **Proposed (2026-11-26)** (ANR-806, part 2; built in `orchestrator/crates/app/src/provisioning.rs`, off unless the deployment has a `provisioner` section). Not yet tried against a real platform: the dev stack plays it with a WireMock stand-in (`dev/provisioner-e2e.sh`).

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
5. **What this does not do.** *(Amended 2026-11-26, ANR-1061: the message is now sent to the person's own agent, see the note below.)* Sends that do not come from a person's request (the verifier and CI re-delegation, a redelivery, an
   `@mention` or `ask_agent` to a coder) do not provision: the dispatcher does not ask the platform.

## Amended 2026-11-26 (ANR-1061): the message goes to the person's own agent

Decision 5 left the message at the deployment's configured agent, so the agent this ADR makes was never used. It is used now.

- **Seam: the dispatcher's resolution of the agent id to an endpoint** (`Dispatcher::load`), not the thread's target and not the
  registry. The target stays the configured agent (`coder`, `adam`): the gates, the tiers, the tool grants, the permissions and the
  thread the person sees are keyed by it, and changing it would change all of them. The registry already lists the person's agent
  under the name `ensure_ready` returns, so the dispatcher asks it for `<kind>-<owner>` in place of the binding's agent and takes
  its endpoint; `Ctx.agent` is unchanged.
- **Nothing is stored.** The name is a pure function of the thread's owner and the binding's agent (`Provisioning::own_agent_id`,
  the kind read from the id and its aliases, as in decision 2), so every message of a thread reaches the same agent and a fork,
  which keeps its parent's binding agent and has its forker as owner, reaches that person's agent. No migration, no new port.
- **Not listed yet is retried, not trusted.** The platform says `Ready` before its registry may list the agent: the delegation is
  retried with the dispatcher's backoff up to its attempts, and then dead-lettered like an agent the platform removed. It is never
  sent to the shared agent instead: that would answer a person from an agent that is not theirs.
- **A thread begun before provisioning was switched on** has its context at the shared agent; its next message goes to the
  person's agent, which refuses a context it did not make. Turning provisioning on is a change for new threads.
- **Unchanged:** the sends the dispatcher makes on its own that name an agent other than the thread's (`ask_agent`, `@mention`)
  and ask the platform nothing.

## Consequences

- A deployment without a `provisioner` section is exactly as it was: `AppConfig.provisioning` is `None`.
- A person's first message to the coder can take up to the wait-ready bound. A bound that is too short makes first messages
  fail with the 503; one that is too long holds a request open.
- `ThreadStore` gains a provided method, `key_recorded` (default: no), which the in-memory, SQLite and Postgres stores override
  and the conformance suite holds; an implementer that has not written it keeps compiling and treats every request as a first
  one (the commit still refuses the duplicate).
- The 503 is additive to the chat API (`docs/api/chat-api.yaml`).
- Changing the identity claim is a data migration of platform agents (decision 1).

## Alternatives rejected

- **A salted or keyed owner hash.** It would hide the pseudonym from someone who knows a user id, but the key is no secret to
  begin with, and a key that depends on a secret moves every person's agent when the secret rotates.
- **A kind named in the agent's configuration.** More explicit than the alias, but a second place to say what the alias already
  says in the dev stack, and the deployment would have to keep the two equal.
- **Provisioning from the dispatcher.** It would cover every send, but the person would see a message accepted that then waits on
  the platform with nowhere to say so; at the request the answer can be a 503 that writes nothing.
