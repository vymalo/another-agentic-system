# ADR 0091 — A person's memory is off until they turn it on, one switch per agent, and a saved note is stored at once

- **Status:** **Accepted (2026-11-24)**, on the owner's answer to the memory epic's questions 1 and 2. It amends
  invariant 3 of `CLAUDE.md`. Slice M0 of [ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md)
  (the memory epic, [#337](https://github.com/vymalo/another-agentic-system/issues/337)).
- **Owner's decision, ANR-Q59, 2026-11-24 06:29Z:** *(b) "As (a), but notes are saved without asking once memory is
  on"*. Recorded in another-context `decisions-2026-11-24.md` row 6 (`#122`, `810bdd0`). He also answered the second
  question: **memory works through the tools only**, never as a small pinned context.

## Context

[ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md)'s *Memory* section designed a person's
memory and left two questions to the owner: whether a note an agent saves waits for the person's approval, and whether
memory reaches the model as tools or as pinned context. It proposed a `pending` state for a saved note and a per-agent
write mode of `off`, `ask` or `auto`, and it reserved the wording of invariant 3's amendment for *this* decision rather
than for its own.

Both answers now exist, and one of them **removes a state ADR 0067 assumed**: with no approval step there is no `pending`
note and no `ask` mode. What was a design question is now a settled shape, and the contract page and its vectors must
say what the owner decided rather than what was proposed.

## Decision

1. **Off until the person turns it on, with a switch per agent (owner).** Memory is disabled for everyone by default.
   The person enables it **agent by agent**: a standing grant that agent may read, and a standing grant it may write.
   Nothing an agent sees or saves exists until those grants exist.
2. **Once on, a saved note is stored at once (owner).** There is **no approval step**: no `pending` state, no `ask`
   write mode, no waiting note. `memory_save` either stores the note or refuses the call; there is nothing in between.
3. **Tools only, never context (owner).** An agent reaches memory only through the three built-in tools of
   `thread-tools/v1`: `memory_read`, `memory_save`, `memory_forget`. **No note is ever put into a prompt by the
   orchestrator.** The reasons ADR 0067 gives stand: a note pushed into every message would break the prompt cache,
   only what an agent asks for reaches its model provider, and every use is a step the person can read.
4. **Two switches, and both must be on.** The **grant** of decision 1 is the person's standing permission for that
   agent, kept in `user_memory_settings`; the **thread switch** is the per-conversation one, recorded as the additive
   event `memory_switched`. The tools are listed to a thread's **main caller** only when its switch is on **and** the
   agent holds the grant. A child session gets no memory tool at all ([ADR 0072](0072-child-sessions-are-nested-threads-followed-through-child-sessions-v1.md)):
   a helper does not write into its parent's memory.
5. **"Without asking" never means "without knowing" (owner's intent, made concrete here).** The person sees and deletes
   every note through their own routes, `/api/me/memories`, which is **slice M1** — wave 1 of ADR 0067's table, which
   lands **before** the agent tools of **slice M2** (wave 2). Each `memory_save` is also a step of the thread with its
   input and output, as every tool call of `thread-tools/v1` already is. So the order is: the person's page first, then
   the agents that write to it.
6. **The shape, as decided.** Two tables: `user_memories` (a note of at most **1 KiB**, who wrote it, and when) and
   `user_memory_settings` (off by default; for each agent the person enabled, whether it may read and whether it may
   write). **`user_memories` has no `status` column on purpose** — decision 2 removes the only state that would have
   needed one. The routes `/api/me/memories` and `/api/me/memory-settings` need no permission, because they are the
   person's own data ([ADR 0039](0039-nobody-reads-another-persons-thread.md)).
7. **The rules that do not change.** A person's notes belong to that person alone (owner isolation, the first conformance
   case the vectors pin). They are **erased with the person's account**, never copied into a fork or an export, and the
   orchestrator never reads one to reason about anything. Memory is not a thread artifact and does not travel with a
   thread.
8. **What this decision does not settle, and does not need to.** An adam agent's own journal and workspace memory stay
   the agent's business (ADR 0067); nothing here reaches into them. Whether a deployment wants a global kill switch on
   top of the per-person default is a configuration question for M2 and is not decided here.

## The invariant it amends

Invariant 3 of `CLAUDE.md` ("Stateless processes, one event log") gains, dated:

> *Amended by ADR 0091 (accepted 2026-11-24): a person's memory (short notes, at most 1 KiB each, written by the person
> or by an agent they enabled, and which agents may read or write them) is durable state beside the log, the person's
> own, read and changed only through the API. The orchestrator never reads it to reason and never puts it into a prompt:
> an agent reaches it only through a memory tool of `thread-tools/v1` that the person's grants and the thread's switch
> both allow, reported as a step of the thread. It is erased with the account and is never copied into a thread, a fork
> or an export.*

## Consequences

- **A state disappears before it was ever built.** `pending` and the `ask` write mode leave the design, so the migration
  and the vectors of slices M1 and M2 are simpler than ADR 0067 proposed: no approval UI, no expiry for a waiting note,
  no notification for one.
- **The person's control is permissions, not approvals.** A person who does not want an agent to save notes does not
  enable that agent, or turns the thread's switch off; they are not asked per note. This is the trade the owner chose
  explicitly, and it puts the weight on M1's page and on the visible step of every call.
- **The vectors change shape.** The proposed "a note waits for approval" case is gone; the replacement is *"memory on →
  the save is stored at once; memory off → the save is refused"*, which is what `docs/api/examples/memory/` pins.
- **Nothing is built by this decision.** It is the ADR and the contract page only; M1 and M2 are the code.

## Alternatives rejected

- **Approval per note (ADR 0067's own recommendation).** Rejected by the owner: it asks the person to confirm what they
  already enabled, and a waiting note is a state to expire, display and explain.
- **Pinned context in every prompt.** Rejected by the owner: it breaks the prompt cache and sends notes the agent did not
  ask for to a model provider.
- **A single global switch.** Rejected: the person enables the agents they trust, not "memory" as a whole; a per-agent
  grant is what makes a write visible as *that agent's* write.
