# ADR 0089 — The run identity stays the agent's task id, bound when the orchestrator first sees it

- **Status:** **proposed (2026-11-24)** (task ANR-422, a follow-up to the ANR-417 ruling of 2026-11-23). **Nothing is
  built, and the recommendation changes no code.** The orchestrator rules on the recommendation; a different choice is a
  dated note here. Builds on [ADR 0084](0084-the-run-lease-the-broker-binds-a-run-and-lends-its-model-credential.md)
  (the run lease) and adam-rs
  [ADR 0067](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0067-a-model-call-through-an-injector-names-its-run-and-a-refusal-parks-the-run.md)
  (a model call names its run, and a refusal parks the run). **Amended 2026-11-26** by ADR 0101 (accepted 2026-11-26): the run's
  certificate names this id; see the status note of that date. "adam ADR 0067" below is adam-rs's, not this repository's
  ADR 0067.

## Context

**Today the identity is adam's A2A task id, and the orchestrator binds it when it first sees it.** Read on 2026-11-24
in this repository at `797073f9` and in adam-rs at `baef4f61`:

- adam's root run id **is** the A2A task id. adam derives it from the agent, the caller's subject, the request's context
  id and the message id (`crates/adam-a2a-runtime/src/ids.rs`, `task_id_for`). A model call sends that root run id as
  `X-Run-Id`, filled from in-process state, "never from a message's context, which a person can set" (adam ADR 0067,
  decision 2).
- The orchestrator binds the run at the broker (`POST /internal/runs`) the first time the delegation's stream names the
  task id, before it records the message as sent or applies any event (`orchestrator/crates/app/src/dispatcher.rs`,
  `bind_run` at `:618`, called at `:1719`). That is the ANR-417 ruling, option (b).
- A run is bound once. A different binding for a bound run is a `409` (ADR 0084, decision 1, and its status note of
  2026-11-24 for the model pick).
- **The race.** adam may make its first model call before the bind lands. The broker then answers `403`
  `run_not_bound`, and adam **parks the run on a timer**: a backoff of 30 s, doubling, at most 15 min, bounded by
  `MODEL_PARK_LIMIT` (24 h), and then the same call again (adam ADR 0067, decisions 3 and 4). Nothing fails and nothing
  is lost. The cost is a wait.
- adam accepts `adam.resume: true` on a parked run "so the orchestrator can resume a run after binding it" (adam ADR 0067,
  decision 4). **The orchestrator does not send it today.** It has no call site for `adam.resume`.
- The agent is called with one static bearer per agent, shared by every sender
  ([ADR 0064](0064-the-person-reaches-the-agent-and-links-their-github.md), *Context*). So the caller's subject adam sees
  is the orchestrator's, not the person's.

**The question (option (c) of ANR-417):** should the orchestrator choose the run identity itself, carry it to adam with
each delegation, and bind it **before** sending, so that the first model call can never come before the bind?

## Options weighed

| Option | The identity | How it reaches `X-Run-Id` | Consequences |
|---|---|---|---|
| **(b) The agent's task id, bound on first sight** (today) | adam's root run id, `task_id_for` | In-process, as now | Works with no change. The first call may be refused once, and the run waits for the timer: 30 s when the race is lost. Nothing that a caller sends chooses the id. |
| **(c1) The orchestrator's thread id** | The thread's id | A metadata member of every delegation | **Does not fit.** A thread holds many jobs ([ADR 0020](0020-a-thread-is-a-conversation.md)), and two may overlap ([ADR 0036](0036-sending-while-an-agent-works.md)). A run is bound once (ADR 0084, decision 1), so the second job's binding is a `409`, or every job shares the first job's binding: its model endpoint, its expiry, and its revocation when the first job ends, which then stops the second job's calls. |
| **(c2) A per-delegation id the orchestrator mints** | A new id per delegation, for example the job's id | A metadata member of every delegation (an extension like `locale/v1`). adam's LLM agent records it as the root run and sends it as `X-Run-Id` | The bind can come first. But the id now comes from **the message**, which adam ADR 0067 decision 2 forbids: whoever can call the agent chooses which binding its calls spend under. Today that is anyone who holds the agent's bearer. adam would need a list of callers trusted to name a run, and a second meaning of "root run" beside the task id. Two ADRs change, and an agent that does not read the member silently falls back to (b). |
| **(d) The orchestrator computes the task id in advance** | adam's task id, computed by the orchestrator with `task_id_for`'s inputs | In-process, as now | The bind can come first, with no new member. But the orchestrator must know the agent name and the caller subject **as adam derives them**, which is adam's authenticator's private rule. A wrong guess binds a run nobody calls and leaves the real one unbound, which is (b) again with a stale binding. It ties the orchestrator to one agent's id scheme, against [ADR 0007](0007-protocol-only-dependencies.md). |

**Only (c2) and (d) can move the bind ahead of the first call, and each has a cost that (b) does not.** (c2) gives a
caller the choice of the binding. (d) copies a private derivation of adam's.

## What (b) costs, and how to make it cheaper

- **Not measured yet.** Nobody has counted how often the first call loses the race. The bind is one call on the
  cluster's internal network. adam's first model call comes after it starts the task and builds its first prompt. So the
  bind may usually win, but that is a guess.
- **When the race is lost**, the run waits for the first timer, 30 s, and then makes the same call, which now succeeds.
  The task is `auth-required` meanwhile (a park after a `401` or `403`), with the status message "the model gateway has
  not bound this run yet" (adam-rs `crates/adam-a2a-runtime/src/backend.rs:71-90`, a contract of its
  `docs/reference/a2a-server.md`).
- **Two cheap cures exist inside (b), and neither changes the identity:**
  1. **The orchestrator resumes the run after its bind.** When the delegation's stream shows the task `auth-required`
     and the orchestrator's bind of that run has succeeded, the dispatcher sends `adam.resume: true` to the task, once.
     adam ADR 0067 already accepts it, and it adds nothing to the conversation. The wait becomes one round trip. A
     resume to a task that is not parked on the model is refused with `InvalidParams` and writes nothing (adam ADR 0067,
     decision 4), so a resume sent at the wrong moment costs nothing. The state is readable, so the dispatcher never
     matches the status text. This is a change in this repository only.
  2. **adam starts the `run_not_bound` backoff short**, for example at 1 s instead of 30 s, because a bind is expected to
     land within a second. This is a one-line amendment of adam ADR 0067's backoff, for that slug only.

## Recommendation

**Keep (b): the run identity stays adam's task id, bound when the orchestrator first sees it.** Change neither ADR 0084
nor adam ADR 0067's rule that the run id never comes from a message.

1. **Measure first.** Count, at the broker, the `run_not_bound` answers that a bind of the same run follows within a
   minute. That count is the number of races lost.
2. **If the count is not near zero, add cure 1** (the orchestrator sends `adam.resume` after its bind, when it has seen
   the park), as a small slice of this repository with a decisive test. On a mock agent that calls the model before the
   bind lands, the run answers within one round trip of the bind, not after 30 s. Remove the resume, and the test
   waits for the timer and fails its deadline.
3. **Cure 2 is adam's to choose**, and is not needed if cure 1 lands.
4. **Reject (c1)**, because a thread is not a run. **Reject (c2) and (d)** unless the measurement shows that cures 1 and
   2 are not enough. If they are needed, (c2) comes back as a new ADR in both repositories, with the list of trusted
   callers.

**adam-rs needs no ADR for this recommendation.** Cure 1 uses only what adam already offers. adam would need an ADR
only for (c2), which amends decision 2 of adam ADR 0067, or for cure 2, which amends its backoff.

## Consequences

- Nothing changes in the code. ADR 0084 decision 1 ("a run is bound once") and adam ADR 0067 decision 2 (the run id
  comes from in-process state, never from a message) hold as written.
- A run whose first call loses the race waits up to 30 s once, until cure 1 lands.
- The broker gains one counter (step 1), which names no person and no value.

## Alternatives rejected

The thread id (c1), a minted id in the message (c2), and an id computed in advance (d), for the reasons in
[the options](#options-weighed).

### Status note, 2026-11-26: the run's certificate names the same id ([ADR 0101](0101-a-cloud-runs-model-calls-are-signed-for-the-run-and-the-injector-stays-for-custom-cases.md))

ADR 0101 (accepted 2026-11-26, on the owner's direction of 2026-11-26) gives each cloud run a certificate of its own whose URI SAN
names the person and **this record's run id**: adam's task id, bound on first sight. The recommendation above stands,
and option (b) is now also the identity a key is issued for:

- **The race moves to the certificate.** The signer asks the broker for the run's certificate at the run's first model
  call; before the bind it is refused `run_not_bound` and parks exactly as a refused model call does today (adam-rs ADR
  0067). Cure 1 (`adam.resume` after the bind) shortens that wait for the certificate as it does for the lease, and the
  measurement of step 1 counts the certificate refusals beside the lease's.
- **The run id is no longer a capability on the signed path.** The gateway's verifier requires the leaf's run to be
  `X-Run-Id`, so naming a run without its key spends nothing. Options (c2) and (d) gain nothing from this and stay
  rejected.
