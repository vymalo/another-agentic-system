# ADR 0018 — Configurable verification gate and a bounded rework loop

- **Status:** accepted (2026-09-30). **Built:** MVP slice 2 (the core), slice 3 (configuration and
  the AG-UI projection) and slice 4 (the web), 2026-09-30, for the agent-checks source; see *Built
  (slice 3)* and *Built (slice 4)* below; and slice 10, 2026-09-30, the verifier agent (see *Built
  (slice 10)* below). Slices 5 and 6 (the inbox, and the CI webhook that makes `ci` a source this build honours) are
  built too, see *Updated (slice 6)*.
  **Amended 2026-09-30 (review of slices 6, 7 and 9):** a gate that requires `ci` must name its
  checks, and `ci` is honoured only where a webhook is mounted, see the
  [status note](#status-note-2026-09-30-review-fixes); "the first completed CI report decides" below is superseded.
  **Amended 2026-09-30 (the first live run):** the agent's own checks pass only on the pushed commit, see the
  [status note](#status-note-2026-09-30-the-agents-checks-need-a-pushed-commit), which also has the rework prompt
  carry the person's messages, writes down the fence grammar of the prompts the core writes, and lists the holes that
  remain.
  **Amended 2026-09-30 (threads never lock):** the gate applies to **each job** of a thread; verifications are
  counted per thread, see the [status note](#status-note-2026-09-30-threads-never-lock).
  **Amended 2026-10-04 (the chat that is not a pull request):** the gate verifies **only pushed work**: an agent that
  finishes its first attempt with no `branch` artifact gave an answer, and the job is done, see the
  [status note](#status-note-2026-10-04-only-pushed-work-is-verified), which replaces the 2026-09-30 rule that failed it.
  **Amended 2026-10-07 (the owner's exports of 2026-10-06):** pushed work is verified in **every** attempt and job
  (a rework that exists only because a `branch` artifact could not be used is no longer a trap), "no pushed commit" is
  said before "no checks reported", and a failing check that fails on the base commit too (`preexisting: true`) passes
  with a note, see the
  [status note](#status-note-2026-10-07-pushed-work-is-verified-in-every-attempt-and-a-failure-on-the-base-is-a-note).
  **Planned, not built:** the web's card for CI (slice 8)
  ([`mvp.md`](../mvp.md#the-slices-of-steps-2-3-and-6)).
  Refines [ADR 0002](0002-verification-over-consensus.md) (how "verify" and "budgets" are made
  concrete). Closes [open question 8](../open-questions.md#closed) and answers part of question 6
  (attempts; wall clock for the waits on CI and the verifier). Builds on
  [ADR 0016](0016-inbox-timers-and-job-ledger-on-the-thread.md) (the ledger and the loop's rules)
  and [ADR 0017](0017-ci-results-by-webhook.md) (CI results).

## Context

[ADR 0002](0002-verification-over-consensus.md): an external judge decides quality, the loop is
work, verify, rework with findings until green, bounded by budgets, and a job never ends "done"
while its checks are red. MVP step 3 is that loop. Open question 8 asked where verification runs:
real CI arriving by webhook, or a verifier agent over A2A; the proposed answer was to gate on real
CI and allow a verifier for inner loops.

The owner decided on 2026-09-30:

- **The gate is configurable** over three sources: CI on the pushed SHA, checks the agent reports
  itself, and a verifier A2A agent.
- **Attempts:** 3 by default, configurable.

adam-coder does not report its checks today. *Verified 2026-09-30* (adam-rs `882e239`): it emits
`branch` and `pull_request` artifacts, and `run_checks` emits **no** artifact. The agent-checks
source therefore needs a cross-repository change (a `checks` artifact), approved as part of this
choice. Until it lands, the coder's dev gate is CI only.

## Decision

### Sources, ports, aggregation

The sources are a closed enum, `CheckSource`: `Ci`, `AgentChecks`, `Verifier`. **There is no new
port**: the aggregation (which sources are required, which have passed, whether to rework) is pure
core logic in `orch-core`, the rules of [ADR 0016](0016-inbox-timers-and-job-ledger-on-the-thread.md#3-the-loop-as-rules-of-the-core).

| Source | Evidence | Arrives as | Passes when |
|---|---|---|---|
| `Ci` | Reports on the pushed SHA ([ADR 0017](0017-ci-results-by-webhook.md)) | `Input::CiReported` | the `CiPolicy` says so: all required names pass. *(The first completed report used to pass when no name was required; superseded 2026-09-30, see the status note.)* |
| `AgentChecks` | The agent's `checks {passed, commit, summary?, findings?}` artifact | the artifact, recognised in the core | `passed` is true **and** a commit was pushed **and** the checks ran on exactly that commit. *(Without a pushed commit they used to pass; superseded 2026-09-30, see the second status note. Since 2026-10-04 the source is asked only about pushed work: with no `branch` artifact in the first attempt the gate does not apply at all, see the [last status note](#status-note-2026-10-04-only-pushed-work-is-verified).)* |
| `Verifier` | A verifier A2A agent's `verdict {passed, findings[]}` artifact | `Input::VerifierReported` | `passed` is true |

Two rules from the loop that matter here:

- A required source with nothing to check counts as failed rather than pending forever. With no
  pushed SHA, CI and the verifier fail with "no pushed commit". With `AgentChecks` required and no
  `checks` artifact by the time the agent completes, it fails with "no checks reported". *(Since 2026-09-30 the
  agent's checks need a pushed commit too: with none they fail with "no pushed commit", like the other two. Since 2026-10-04
  that failure is for a rework that pushed nothing and for a `branch` artifact the gate could not use; an agent that
  pushed nothing in its first attempt is not asked at all, see the
  [last status note](#status-note-2026-10-04-only-pushed-work-is-verified).)*
- A failed source ends the round at once: the thread reworks (or fails) without waiting for the
  others. A result that arrives later for an older SHA or attempt is recorded and changes nothing.

### The verifier

- `RequestVerification` becomes a new **outbox kind, `verify`**, with a new column
  **`outbox.task_id`** (migration `0003`, slice 2). The dispatcher sends to the verifier's endpoint
  in a context of its own, `<thread>-verify-<attempt>-<verification>` (*amended 2026-09-30, slice 10:
  the text first said `<thread>-verify-<attempt>`; see Built (slice 10)*), so the verifier's A2A context
  is separate from the worker's.
- The dispatcher **never maps a verifier's envelopes to `Input::Agent`**: a verifier is not the
  worker, and its "completed" must not complete the job. It emits exactly one
  `Input::VerifierReported`, from a `verdict` artifact `{passed, findings[]}`. No verdict is a
  failed check with "no verdict".
- A verifier failure, or `ORCH_VERIFIER_TIMEOUT_SECS`, puts the thread in **`Blocked`**, not
  `Failed`: the verifier being down is not the code's fault, and it does not spend an attempt.
- The verifier is another configured agent (`AGENTS_FILE`); it never receives the worker's
  credentials.

### Configuration

Three layers, from widest to narrowest. Each layer may only tighten the one above.

| Layer | Where | What |
|---|---|---|
| Deployment | env | `ORCH_GATE` (comma list, e.g. `ci,agent-checks`; **default empty**, which is today's behaviour), `ORCH_MAX_ATTEMPTS` (`3`), `ORCH_MAX_ATTEMPTS_CAP` (`10`), `ORCH_VERIFIER` (agent id), `ORCH_VERIFIER_TIMEOUT_SECS` (`1800`), `ORCH_CI_TIMEOUT_SECS` (`3600`, [ADR 0017](0017-ci-results-by-webhook.md)) |
| Target | an `AGENTS_FILE` entry | `gate: {require: […], maxAttempts, verifier: <agent id>, ci: {required: […], timeoutSecs}}`, parsed with `deny_unknown_fields`. The verifier must be another configured agent, checked at startup (a bad reference is exit 78) |
| Thread | AG-UI `forwardedProps["vymalo.gate"]`; MCP `start_job.gate` | A thread may **add** sources and change attempts up to `ORCH_MAX_ATTEMPTS_CAP`; it may **not remove** a source its target requires |

The gate in force is resolved when the thread is created and copied into `Job`
([ADR 0016](0016-inbox-timers-and-job-ledger-on-the-thread.md)); a running job never sees a later
configuration change.

Defaults chosen (the owner's decisions, and the plan's defaults): the gate is empty; 3 attempts,
capped at 10; a timeout blocks the thread rather than spending an attempt; without `ci.required`,
the first completed CI report decides *(superseded 2026-09-30: a gate that requires `ci` must name its checks, see the status note below)*; a per-thread gate can add sources but not remove them.

### Findings

- Findings are capped at **20 items and 16 KiB per source**.
- They are **quoted as untrusted data** in the rework prompt (delimited, and labelled as coming
  from a check, not from the user): a CI summary or a verifier's text is input from outside.
- The findings text is built in the core, so it is the same on every replica and in every replay.

### Projection to the chat

AG-UI ([ADR 0012](0012-ag-ui-user-facing-protocol.md); the mapping tables in
[`api/agui.md`](../api/agui.md) gain these rows when built):

- A run stays open while the thread is `Queued`, `Working` or `Verifying`.
- `completed` into verifying emits `SUBAGENT_FINISHED` and a `STATE_SNAPSHOT` (the thread plus
  `job {attempt, maxAttempts, gate, sha}`), **not** `RUN_FINISHED`.
- `check_result` becomes the activity `vymalo.check`.
- `rework` becomes `vymalo.rework`, then `SUBAGENT_STARTED` for the next attempt.
- The verifier appears as its own subagent.
- `Done` gives `RUN_FINISHED` with success. Out of attempts gives `RUN_ERROR` with
  `code: "checks_failed"`.

`chat-api.yaml`: `Thread.state` gains `verifying`, plus an optional `job`. Two new goldens,
`verify-green` and `verify-red`, are read through the reference client in CI. The web shows a
`verifying` badge, an attempt counter such as "2/3", findings per source and a "reworking" divider.

### Diagrams

One job, from the agent's completion to green, through one rework:

```mermaid
sequenceDiagram
  participant W as Worker agent (A2A)
  participant O as Orchestrator (transition, pure)
  participant CI as CI (webhook)
  participant V as Verifier agent (A2A)
  participant U as Chat
  W-->>O: artifacts branch{sha1} and checks, then completed
  O->>O: gate requires ci and verifier: Verifying, Watch ci:repo@sha1
  O-->>U: check_result pending (ci), pending (verifier)
  par CI
    CI-->>O: CiReported sha1 = failure
  and verifier
    O->>V: verify (context thread-verify-1-1)
    V-->>O: verdict {passed: true}
  end
  O->>O: ci failed, attempt 1 < 3: rework
  O-->>U: check_result failed (findings), rework 1 of 3
  O->>W: Delegate with the findings, quoted as untrusted
  W-->>O: artifacts branch{sha2} and checks, then completed
  O->>O: Verifying at attempt 2, Watch ci:repo@sha2
  CI-->>O: CiReported sha2 = success
  O->>V: verify (context thread-verify-2-2)
  V-->>O: verdict {passed: true}
  O->>O: all required sources passed: Done
  O-->>U: RUN_FINISHED success
```

```mermaid
stateDiagram-v2
  [*] --> Queued
  Queued --> Working: the agent reports working
  Working --> Done: completed, gate requires nothing
  Working --> Done: completed, nothing pushed (an answer, 2026-10-04 and 2026-10-07)
  Working --> Verifying: completed, gate requires sources
  Verifying --> Done: every required source passed
  Verifying --> Queued: a source failed, attempt < max (rework, attempt + 1)
  Verifying --> Failed: a source failed on the last attempt
  Verifying --> Blocked: ci_timeout, verifier failure or verifier timeout (no attempt spent)
  Verifying --> Queued: a user message (no attempt counted)
  Verifying --> Cancelled: cancel
  Blocked --> Queued: a user message
  Done --> [*]
  Failed --> [*]
  Cancelled --> [*]
```

`Queued` and `Working` also reach `Blocked`, `Failed` and `Cancelled` exactly as in
[Thread state](../architecture.md#thread-state); the diagram shows only what the gate adds. What it
cannot say: a `Blocked` thread that the user answers is delegated again **without** a new attempt
number, because the timeout was not the worker's failure; an out-of-attempts `Failed` carries the
last findings in its `error` event.

## Built (slice 3)

*2026-09-30.* What the code does, where it refines the text above:

- **Configuration** is one rule set, `GateRules` in `orch-app` (`gate_config.rs`): the binary applies the
  deployment variables and every `AGENTS_FILE` entry with it at startup, and `App` applies the entry and the
  request when a thread is created; `App::new` checks the deployment's policy and every entry again, so another
  composition root cannot hand it a gate the build cannot honour (it returns an error; the binary exits 78). A layer is
  `{require?, maxAttempts?, verifier?, ci?}`. What "each layer may only tighten the one above" comes to in code:
  sources can only be added (`require` is the whole list and must contain the one above's; `ci.required` names
  add up), and attempts can be anything in `1..=ORCH_MAX_ATTEMPTS_CAP` (at most 100), lower or higher than the
  layer above's. The one removal allowed is the entry of the verifier itself leaving the `verifier` source out for
  itself, because an agent cannot verify its own work; an agent whose own gate does not require the verifier may be
  the verifier. A thread may set only `require` and `maxAttempts`, and spells a source `agent-checks` or
  `agent_checks`. A default `ORCH_MAX_ATTEMPTS` is lowered to a smaller `ORCH_MAX_ATTEMPTS_CAP`; one that is set must fit.
  The verifier of every agent's resolved gate must be a configured agent.
- **This build honours only `agent-checks`** *(as slice 3 left it; it honours `verifier` as well since slice 10, see Built (slice 10), and `ci` since slice 6, see Updated (slice 6))*. The application still drops `RequestVerification` (slice 10; `Watch` and `Schedule` are executed since slice 5, but no
  surface writes CI reports until slice 6), so a gate that required `ci` or `verifier` would wait for a verdict
  that never comes. Configuration **refuses** those sources, and the `ci` and `verifier` settings, in every layer,
  and says which slice enables them: startup exits 78 (`ORCH_GATE`, `ORCH_VERIFIER`, an `AGENTS_FILE`
  entry), a request is a 400. This is fail-closed: a job is never "done" without a check the operator required.
  `pending_reason` in `gate_config.rs` is where the sources are listed, and `GateRules::honouring` how a build (or a
  test) says it has more. It is **not** the only thing a later slice changes: the slice that makes `verifier` real also
  owns its checks (`GateRules::check_verifier`, the verifier's own entry) and its cards, and the one that makes `ci` real
  owns the `ci` settings and the watch keys.
- **Not built yet:** `ORCH_CI_TIMEOUT_SECS` and `ORCH_VERIFIER_TIMEOUT_SECS`, which only matter once CI and the
  verifier can be required (slices 6 and 10); the per-target `ci.timeoutSecs` is accepted by the parser and refused
  with the rest of the `ci` settings. *(Update 2026-09-30: slice 10 reads `ORCH_VERIFIER_TIMEOUT_SECS` and honours the
  verifier, see below; the rest of this paragraph and the one before it are as slice 3 left them, for the `ci` source.)*
  *(Update 2026-09-30: `ORCH_CI_TIMEOUT_SECS` and the per-target `ci.timeoutSecs` are read since slice 6, see Updated (slice 6).)*
- **Projection:** as above. The gate reaches the projection through `ThreadMeta.gate` (the job's copy, fixed when the
  thread was created); everything else is folded from the log. The `vymalo.check` card of a source in an attempt
  has a stable id, `check-<attempt>-<verification>-<source>`, and is `replace: true` (a second verification of the
  same attempt is a card of its own); `vymalo.rework` is `rework-<attempt>`; a rework opens the next attempt's
  subagent at once. `job` is in `STATE_SNAPSHOT` and in `Thread` only under a gate, and its `sha` is the pushed
  commit, never the commit a check ran on. A hold (`error{retryable}` then `blocked` while verifying) is an
  answerable interrupt, not `delivery_failed`.
  Goldens: `verify-green` (red once, then green), `verify-red` (three attempts, `checks_failed`).
- **A rework is a new A2A task in the same context**, because the first task is `completed`; `orch-e2e` pins it.
- **The gate is fixed at creation**, and a run that continues a thread and asks for another is a 409 (`orch-surface-agui`),
  not a silent no-op.

## Built (slice 4)

*2026-09-30.* The web (`web/`) renders the gate; it invents nothing, so each thing below is read from the stream:

- **The badge** has a `verifying` state (its own colour and label, "Verifying", spoken as "Verifying the agent's
  work"), and the composer offers Cancel while it lasts.
- **The attempt counter** ("Attempt 2/3", spoken as "Attempt 2 of 3") sits beside the badge whenever the newest
  `STATE_SNAPSHOT` (or, before the stream, `Thread.job`) carries a `job`; a thread without a gate shows none.
- **`vymalo.check`** is a card per source and attempt (status in words, source, short commit, summary and findings),
  replaced in place by its id; a `stale` one is its own muted card. **`vymalo.rework`** is a divider, "Attempt 2 of 3:
  sent back with 1 finding". Findings, summaries and names are untrusted and drawn as text: never as markdown or HTML;
  a long finding is cut with an expand control.
- **`RUN_ERROR` `checks_failed`** is kept by the stream's agent and shown as "Checks failed after 3 attempts" where an
  ordinary finished thread says "This thread is failed".
- The mock replays both goldens (and `verify-pass`, and the mock-only `verify-ci` and `verify-wait`, which need the CI
  source of slices 5 and 6); the system tests run the fake agent's `verify-*` scripts through the real orchestrator.
  Rules and tests: [`web/README.md`](../../web/README.md#verification-the-gate).
## Updated (slice 6)

*2026-09-30.* **This build honours `ci` and `agent-checks`; `verifier` is still refused until slice 10.**
`pending_reason(CheckSource::Ci)` is `None`: the CI webhook ([ADR 0017](0017-ci-results-by-webhook.md)) writes
reports into the inbox that slice 5 built, so a gate that requires `ci` can be decided. The `ci` source and the `ci`
settings (`ci.required`, `ci.timeoutSecs`) are accepted in the deployment (`ORCH_GATE`, `ORCH_CI_TIMEOUT_SECS`) and in
an `AGENTS_FILE` entry, and refused per thread as before (`ci` cannot be set by a request; `require: [ci]` can be
added by one). `verifier` and its setting are refused in every layer with the slice that enables them. Whether a
deployment can *receive* reports is `ORCH_SURFACES`: a `ci` gate with no webhook mounted is not refused (another
replica group may serve the webhooks), it ends `Blocked` (`ci_timeout`) after the timeout, and a control plane warns
at startup.

## Built (slice 10)

*2026-09-30.* The verifier is a real source. What the code does, where it refines or departs from the text
above:

- **The request.** `RequestVerification` is an outbox row of kind `verify` (`OutboxPayload::Verify`: the attempt,
  the verification, the verifier's id, what was pushed and the prompt the core wrote). It is written in the same
  commit as the `Verifying` transition and the `VerifierDeadline` timer, so it cannot be lost or made twice. A
  `verify` row is **not ordered** like a delegation: it is not held back by the delegate whose completion caused it
  (still being finished), and it holds back none.
- **The context is `<thread>-verify-<attempt>-<verification>`**, not `<thread>-verify-<attempt>` (a *deviation*
  from the text above, which had only the attempt). A user who writes during a verification, or a hold the user
  answers, makes the worker finish again in the same attempt, and the verification that follows is a new one: it must
  not land in the A2A context of a verification that is over. The job's `verification` counter is unique per job,
  so the context is unique too. `orch_core::verifier_context` builds it.
- **What the verifier is sent** (the core writes it, `verifier_prompt`): the commit (a hash, the one thing outside a
  fence), the attempt ("attempt 2 of 3"), how to answer (a `verdict` artifact), and three quoted blocks, each in a
  fence the text cannot close and labelled as data, not instructions to the verifier: where the worker says it pushed
  the commit (its repository and its branch), the user's messages in the order they wrote them (the job's `task`, see the status note of 2026-09-30), and what the worker said about its work
  (its last final message in this attempt; the text of its `completed` replaces that when it is not blank; at most
  4 KiB; `Job.summary`, kept only under a gate that requires the verifier and forgotten at a rework). Everything the
  worker controls is inside a fence, and the branch is also refused unless git would accept it
  (`git check-ref-format --branch`: no whitespace, control characters or any of `` ` `` `~` `^` `:` `?` `*` `[` `\`, no
  `..`, `@{`, empty component, leading `-`, trailing `/`, `.` or `.lock`, at most 255 bytes); an artifact with another
  branch is logged as malformed and pushes nothing. The repository is the ledger's normalised key (`host/owner/name`,
  no scheme: what `repo_key` makes of the address the worker reported), not the address as the worker wrote it; a
  verifier that needs a clone URL builds it from the host. It never receives the worker's credentials: it is another
  configured agent with its own `tokenEnv`.
- **The task is on the row.** `outbox.task_id` holds the verifier's A2A task once its first envelope arrived
  (`ThreadStore::mark_verify_sent`, fenced by the claim and never touching the thread's binding, which is the
  worker's). A re-claimed row re-attaches to that task (`resubscribe`, then `get_task`); one that crashed between
  sending and recording looks the message up by its id (`find_task_by_message`, tried up to three times when the
  lookup itself fails) and sends the request again only when the lookup answers that there is no such task. **The
  verifier is asked at most once when it supports `ListTasks`;** otherwise (the lookup cannot be answered) the row is
  retried and, in the end, the thread is held for the user rather than asking again, and the unique context and
  `messageId` are what lets a verifier that saw the request twice tell. A resubscription yields the rest of the
  stream, not what came before it, so a task that completes there without a verdict is read with `get_task` (its
  artifacts) before "no verdict" is concluded: a verdict streamed before a crash is not lost.
- **Envelopes are read, not applied.** The dispatcher's verify path reads the verifier's stream and never maps it to
  `Input::Agent`. It keeps the latest `verdict` artifact and, when the task ends its turn, feeds exactly one input:
  `Input::VerifierReported` when it completed (no `verdict` is `Verdict::missing()`, one that cannot be read
  (`passed` not a boolean, `findings` not a list, not JSON, over 256 KiB) is `Verdict::unusable(why)`: both fail the
  check, fail closed, and say "no verdict"), or `Input::VerifierFailed` when it failed, was rejected or cancelled, asked
  for input nobody can give, could not be reached after the delegation's retries, refused the request, or is no
  longer configured. **`VerifierFailed` is new** (*a deviation*: the text had the dispatcher reuse `DeliveryFailed`,
  which carries no verification, so a late failure of a verification that is over could have held a thread that had
  moved on). It names the attempt and the verification like the verdict does, and holds the thread
  (`Hold::VerifierFailed`, `Blocked`, no attempt spent) only when that verification is the one waiting.
- **Findings are capped where they are read** (`parse_verdict`: 20 items and 16 KiB, a finding that is not a string is
  kept as its JSON) and again where the verdict is applied, and they reach the worker only quoted as untrusted.
- **Stale answers** change nothing except a stale `check_result`: the verdict is applied under the key
  `verdict:<row>` (one verdict is one event whoever applies it, a replay is `Duplicate`), carries its attempt and
  verification, and the core compares both. The row also watches its thread (`DispatcherConfig::verify_watch`,
  5 s): when the verification is no longer the one in progress (a timeout, a cancel, a message from the user, another
  source that already failed the round) it asks the verifier to stop (`CancelTask`, best effort, when it has the
  task's id) and ends the row `skipped`, so a verifier that hangs does not hold a worker for ever. The look happens
  between two envelopes or two polls, never in the middle of a write: a request that has been sent is recorded
  before anything else, and a verdict that is being committed is finished, not cancelled. `ORCH_VERIFIER_WATCH_SECS`
  (5, at least 1) sets how often.
- **The wait is bounded by the core's timer.** `ORCH_VERIFIER_TIMEOUT_SECS` (1800, at least 1; `GatePolicy`'s
  `verifier_timeout`, deployment-wide) is armed as a slice 5 timer by the `Schedule` the core emits with the request;
  when it fires the thread is `Blocked` with `Hold::VerifierTimeout`, the attempt is not spent and the row is dropped
  as above.
- **Fencing.** Every write of the path is fenced with the outbox claim, like the delegation path: recording the send,
  the retries, the verdict or failure (`App::apply` with the lease), the end of the row. A worker that lost its claim
  writes nothing; `orch-app`'s tests pin it with a claim taken over while the verifier answers.
- **Configuration.** `pending_reason` no longer refuses `verifier`: `ORCH_GATE` may name it, `ORCH_VERIFIER` and the
  `verifier` key of an `AGENTS_FILE` `gate` are read, and `ORCH_VERIFIER_TIMEOUT_SECS` is read; the other rules of
  slice 3 stand (a verifier must be a configured agent, a thread may require it but not choose it, and an agent whose
  resolved gate requires the verifier cannot be the verifier itself, whether by the same id or by another id for the
  same card URL, unless its own entry leaves the source out). A
  request that requires the verifier is checked against the same rules once it is applied, so a thread that asks for it
  where no verifier is configured, or on the verifier itself, is a 400 before anything is created (not a job that fails its
  check later); the MCP server's `start_job.gate` goes through the same `App::resolve_gate`, so it is refused the same way.
  `ci` stays refused, with its settings, until slices 5 and 6 of the CI plan land.
- **Projection.** The verifier is a subagent of its own, named after the verifier agent, with the stable id
  `sub-verify-<verification>`: it starts with the `pending` card of the verifier source and ends with its verdict
  (`SUBAGENT_FINISHED` with `result: {"passed": …}`), or, when the verification ends without one (the round was decided
  by another source, the user wrote, the thread was cancelled), with `result: {"status": "canceled"}`, or, when the thread
  was held while the verifier was out and no CI is required (so the hold can only be the verifier's: it failed, was
  refused, or was too slow), with `SUBAGENT_ERROR` (`code: "verifier_failed"`, the hold's message). A client
  that joins while the verifier is out is told about its subagent in the preamble. The verdict is a `vymalo.check` card
  with source `verifier`. Goldens: `verify-verifier-green` (findings once, then green) and `verify-verifier-red`
  (three attempts, `checks_failed`).
- **Not built.** When CI is a required source too, a timeout or a failure of the verifier is still shown to its
  subagent as cancelled, not as failed: the projection cannot tell from the log which source the hold was about, and the
  interrupt says why. CI remains unbuilt; `AgentChecks` still needs adam-coder's `checks` artifact (cross-repository).
- **Dev stack.** `mock-verifier` (WireMock; findings for a commit of forty `a`, a pass for any other), the coder's
  `push-flawed` and `push-clean` keywords, `verifier` and `mock-coder-verified` in `dev/agents.yaml`, and
  `dev/verifier-e2e.sh`.

## Status note (2026-09-30): review fixes

- **`ci` requires `ci.required` names.** Replaces "without `ci.required`, the first completed CI report decides"
  (Decision, *Defaults chosen*) and the `Ci` row of the sources table. With no names the first report for the pushed
  commit decided, so a `skipped` report of another check, another workflow's, or a fork's, passed a red commit
  ([ADR 0017](0017-ci-results-by-webhook.md#status-note-2026-09-30-review-fixes)). `GateRules` now refuses, as a
  `GateError::CiWithoutChecks`, a resolved policy that requires `ci` and names no check: the deployment
  (`ORCH_GATE` without `ORCH_CI_REQUIRED`) and every `AGENTS_FILE` entry at startup (exit 78), a per-thread `require`
  that adds `ci` on a policy with no names as a 400 (or a tool error over MCP). Names union across layers as before, so a
  layer cannot drop the ones above. The core is fail-closed too: with none named, no report counts and the CI deadline
  blocks the job.
- **`ci` is honoured only with a webhook surface.** `GateRules::refusing(Ci, reason)` is how a deployment refuses a
  source of its own: the binary uses it when a process that serves routes mounts neither `webhook-generic` nor
  `webhook-github`, with the reason "no CI webhook surface is mounted (ORCH_SURFACES)", for the deployment, for agents
  and for per-thread requests. Replaces the startup warning of slice 6 ("a `ci` gate with no webhook mounted is not
  refused"). A `worker` serves no routes and honours what the control plane decides.
- **Configuration.** `ORCH_CI_REQUIRED` (comma-separated check names) sets the deployment's `ci.required`.
- **The `vymalo.check` card of source `ci`** carries the summary of the check when exactly one is named.

## Status note (2026-09-30): the agent's checks need a pushed commit

*Why.* On the owner's first live run (real model, real GitHub, `dev/agents.live.yaml`, the coder gated on
`agent-checks`) the owner typed "Hi". The coder answered in plain text and completed; the gate sent it back with "no
checks reported". On attempt 2 the model invented a task on a repository nobody asked for and ran `run_checks` on the
**unchanged** worktree. That produced a passing `checks` artifact bound to the base `HEAD`, no `branch` artifact and
no pushed commit, and the thread ended **Done** ("Attempt 2/3"). Nothing had been produced, and the gate called it
finished.

*What was wrong.* `agent_checks` compared the checks' commit with the pushed one only when **both** existed. With no
pushed commit, or with checks that named none, the comparison was skipped and the checks' own `passed` decided. That
contradicts [ADR 0003](0003-git-as-durable-state-ephemeral-workers.md) (git is the artifact: what the agent did is the
commit it pushed) and this ADR's own table ("`passed` is true for the pushed commit"), and it was the one source of
the three that did not fail closed on a missing push. CI and the verifier already did.

*The rule now.* A job gated on `agent-checks` is done only when the checks passed **on the pushed commit**:

| What the job holds when the agent finishes | The source says |
|---|---|
| no `checks` artifact | failed: "no checks reported" (unchanged) |
| checks, but no pushed commit | failed: "no pushed commit: the agent reported no `branch` artifact, so there is nothing to check", the same finding CI and the verifier give; when the agent did send a `branch` artifact that the gate could not use (a short hash, a repository that is no address, a branch git refuses), the finding is instead "the `branch` artifact was not usable: <reason>" (`Job.branch_problem`, cleared by a usable `branch` and by a rework), so the agent hears what to fix; if the checks themselves failed, their findings follow it, so the agent sees both. A refused report's own summary is not shown (a "42 tests pass" beside a failure reads as praise) |
| checks and a pushed commit, but the checks name no commit | failed: "the checks name no commit, so they cannot be tied to the pushed commit <sha>" (an unreadable `checks` artifact keeps its own reason, which already fails) |
| checks that ran on another commit than the pushed one | failed: "the checks ran on commit A but the pushed commit is B" (unchanged) |
| checks that passed on the pushed commit | passed |

*(Superseded 2026-10-04 for a first attempt that pushed nothing, see the [last status note](#status-note-2026-10-04-only-pushed-work-is-verified): the table below is what the gate says of an agent that pushed, tried to push or is being reworked.)*

A failed source reworks while attempts are left, so the owner's "Hi" now ends in a rework telling the agent to push
its work, and after the last attempt in `Failed` with that finding, never in `Done`. *(This was the 2026-09-30 reading;
2026-10-04: the "Hi" is an answer and ends `Done` on attempt 1.)* `transition` stays a pure function:
the change is in `verify::agent_checks`, which reads only the job.

*The rework prompt carries what the person wrote (same day, same run).* On attempt 2 of that run the coder said that "the
task text itself was never carried into this session; the feedback contained only the check-result complaint". It was
right: each attempt is a **new A2A task** ([Decision](#decision), the rework), and an agent need not remember the one
before (the coder keeps no memory across tasks), but `rework_prompt` sent only the findings. The prompt now opens as
before ("Your work did not pass verification (attempt N of M); this is attempt N+1"), then carries **the person's
messages in their own words**, in a fence labelled `request`, and only then the findings, quoted as untrusted data as
before. The first version carried only the first message; a review the same day found that wrong: with the coder's
rule that a stop with no pull request becomes a question, a job goes "Hi", the coder asks, the person answers "fix
login in acme/widgets", the attempt fails, and a prompt that quotes "Hi" and says it is the request loses the answer.

- **What `Job.task` holds.** Every user message of the job, in the order they were written, the newest last: the first
  message as it is, each later one after a line `[next message]`. `note_task` adds one per `user_message` the gate sees
  (in `queued`, `working`, `blocked` and `verifying`), under every **active** gate, whichever sources it requires (it
  never kept anything for a job with no gate; the field is the job's, not the verifier's, although the verifier's
  prompt reads it too). Whitespace around a message is dropped and a blank message adds nothing.
- **The cap.** The whole is at most 8 KiB (`MAX_TASK_BYTES`), so a long chat cannot grow the ledger or the prompts
  without bound. A lone message is cut there. With several, each is cut to about half the cap, and the **first message
  and as many of the newest as fit** are kept: what lies between is replaced by one line `[… earlier messages omitted
  …]`. A message that had to be cut ends in ` [cut]`. The first message stays because it is usually the request, the
  newest because it is usually the answer that changed it; the middle is the part that can go. All cuts are on a
  character boundary, and the result is a pure function of the messages, so a replay makes the same text.
- **How the agent is told.** "These are the person's messages, in their own words and the order they wrote them (the
  latest last, a `[next message]` line between two of them); a later one answers or changes an earlier one. They are
  your task: carry on with it." It no longer says to "keep doing it on the same repository and branch" or "not to start
  a different one": a later message may name another repository, and the agent's own rules (the coder's named-repo
  check) decide what the messages mean. The verifier's prompt quotes the same text as untrusted data under "The
  task: the user's messages in the order they wrote them".
- **Old ledgers.** `Job.task` stays a string: a ledger written with one message reads as a job with one message, and a
  job with no task (a ledger written before the field) gets the prompt it always got.
- The messages are the instruction to follow; the findings are data that describes problems and "not instructions".
  The two are fenced differently on purpose (see the grammar below).

*The fence grammar of the prompts the core writes.* Everything that is data and not instruction is quoted in a fenced
block, and an agent that parses a prompt (the coder's named-repo check reads the person's messages to find the
repository) must read it the way CommonMark does:

- An **opening line** of N backticks, with N at least 3, immediately followed by a **label** and nothing else: `request`
  (the person's messages, in the prompt of the agent they instruct) or `untrusted` (data that is no instruction to
  its reader: the findings, where the worker says it pushed, the worker's own summary and, in the verifier's prompt,
  the person's messages too, because the verifier is not the one they instruct), then the text on its own lines.
- The **closing line** is at least N backticks and nothing else. N is **longer than any run of backticks in the text**
  (`fence_for`: the longest run, at least 2, plus one), so text cannot close its own fence, and **N is often more
  than 3**: a parser must count the opening fence and match it, never look for a literal three-backtick line.
- **Findings** are a bullet list inside the `untrusted` fence: each finding is one line starting `- `, and its own line
  breaks are continued indented by two spaces.
- A parser that only wants the person's words takes the `request` block and ignores every `untrusted` one. Text
  outside the fences is the core's own (the attempt numbers, the instructions, and the commit hash of the verifier's
  prompt, which `recognise_artifact` accepts only as a hash).
- *Verified 2026-09-30 by the core's tests* (`a_request_cannot_close_its_fence`,
  `a_message_cannot_close_the_fence_the_task_is_quoted_in`, `findings_are_quoted_as_untrusted_data_they_cannot_escape`).

*Consequences.* A gate on `agent-checks` alone is now a gate on "the agent pushed a commit and its own checks passed
on it". An agent that only answers questions cannot sit under it: give that agent no gate (`gate: {}`), as
`dev/agents.live.yaml` says. *(Superseded 2026-10-04: an agent that answers without pushing is done, so the coder
keeps `require: [agent-checks]` and a chat is no longer failed.)* The local stack's agents push a `branch` and report `checks` that name the same commit, and the order does
not matter for that: the gate reads the job when the agent finishes (the fake agent and the WireMock agents send `branch`
first, the coder sends `checks` and then `branch`). Checks that name another commit than the `branch` that follows them
are dropped when it arrives ("facts about another commit no longer count"), and the source then says "no checks
reported". So no scenario changed. Their rework matchers (`this is attempt N`, the scenario keyword, `### the verifier`)
still match the prompt, which quotes the keyword in its `request` block; `dev/check-mocks.sh` builds the current prompts.

*What the gate still does not check.* The rules above make "done" mean "a commit was pushed and the source that looked
at it passed". They do not make it mean "the right commit, in the right place":

- **`agent-checks` is the agent vouching for itself.** It passes when the agent says its checks passed on the commit it
  says it pushed. A model can report both falsely; nothing here looks at the commit. Only `ci` (the commit's own checks,
  reported by the CI system) and the `verifier` (another agent that reads the commit) see it independently, which is
  why a gate on `agent-checks` alone is the weakest of the three and meant for inner loops.
- **The repository is not compared with the request.** `pushed.repository` is whatever the agent reported (put through
  `repo_key`). The orchestrator does not check that it is the repository the person named, or any repository at all
  that the deployment allows. The agent's own rules (the coder's named-repo check) and the verifier are what stand
  between a wrong repository and "done".
- **`commit == base` is not refused.** An agent that reports the commit it started from, with checks that name it,
  passes: the orchestrator does not know the base commit, and a rule that refuses an unchanged tree needs the clone.
  The verifier, which reads the diff, is what catches it.
- **The branch artifact is the agent's word** (its shape is checked, including by `git check-ref-format`; that the
  commit exists on that branch of that repository is not).

## Status note (2026-09-30): threads never lock

[ADR 0020](0020-a-thread-is-a-conversation.md): a message on a finished thread starts the thread's next job, so a
thread runs the gate once per job.

- **The gate is per job, and it is the thread's.** `Job::next()` keeps `gate` (fixed when the thread was created; a
  per-job override is [open question 31](../open-questions.md)) and gives the new job attempt 1, so the budget
  (`max_attempts`) is per job: a follow-up after a job that used all its attempts has all of them again.
- **`Job.verification` is counted per thread, never reset.** A report or timer names `(attempt, verification)`;
  attempt 1 of job 2 must not match attempt 1 of job 1, and because the count only grows, a verdict, a
  `CiDeadline`, a `VerifierDeadline` or a `verify` row of an earlier job is stale by the comparison the core already
  makes. No `job` field was added to any of them.
- **What is cleared** for the new job: `results`, `pushed`, `summary`, `hold`, `branch_problem`, `task` (the new job's
  task is the new message, then accumulates as in the previous status note). A CI report for an earlier job's commit
  is only a card (`about_the_push` compares the new job's pushed commit).
- **Rework is unchanged** inside a job; the verifier still gets no reference to earlier tasks
  ([ADR 0021](0021-context-across-a2a-tasks.md)).


## Status note (2026-10-04): only pushed work is verified

*Why.* On 2026-10-04, live, the coder was gated with `require: [agent-checks]` (`deploy/chart/values.yaml`). Someone asked
it "plot an image in TypeScript and show it here". It built a scratch project, its checks passed, it shared the PNG and
answered. The gate then failed it ("no pushed commit: the agent reported no `branch` artifact, so there is nothing to
check"), reworked it twice (each rework a new task, so the scratch workspace was gone) and ended the thread `Failed`.
The 2026-09-30 rule below it was written for the owner's "Hi", where an agent invented work and called it done; it
closed that hole by treating a missing push as a failed check, and in doing so made every coder chat that is not a pull
request (a question, a demo, "what is this repo about") end `Failed`. That contradicts
[`docs/vision.md`](../vision.md) ("the coder should not push every chat toward code").

*The decision (the owner, 2026-10-04): gate only pushed work.* Git is the artifact
([ADR 0003](0003-git-as-durable-state-ephemeral-workers.md)); a job that left no commit left nothing for a source to
judge. The gate is a judge of commits, not of answers.

*The rule now.* When the agent finishes (`completed`), `verify::is_an_answer` reads the job:

| What the job holds when the agent finishes | The gate |
|---|---|
| no usable `branch` artifact, attempt 1 (whatever else was reported: no checks, passing checks, failing checks, an unreadable `checks` artifact) | **does not apply**: the job is `Done` on attempt 1, nothing is sent back, no source is asked, no timer or watch is set |
| a `branch` artifact the gate could not use (`Job.branch_problem`: a short hash, a repository that is no address, a branch git refuses) | applies: failed with "the `branch` artifact was not usable: <reason>", as before. The agent tried to push and got it wrong |
| a usable `branch` artifact | applies exactly as before: the agent's checks on that commit, CI, the verifier, reworks and the attempt budget |
| attempt 2 or later, no usable `branch` artifact | applies: failed with "no pushed commit", as before (see below). *(Refined 2026-10-07: only when an earlier attempt pushed a usable commit; see the [last status note](#status-note-2026-10-07-pushed-work-is-verified-in-every-attempt-and-a-failure-on-the-base-is-a-note).)* |

*A rework is not an answer.* The fourth row is a refinement of the owner's rule, kept on purpose and cheap to remove: a
job reaches attempt 2 only because attempt 1 pushed, or tried to push, work that did not pass. If finishing that rework
with nothing pushed ended `Done`, an agent could leave a red gate by pushing nothing, and the gate would verify
only the work that is good. So `no pushed commit` still fails a rework, with the finding it always had. It never fails
a first attempt that pushed nothing. A job that was reworked and then stopped by the person starts its next job at
attempt 1 ([ADR 0020](0020-a-thread-is-a-conversation.md)), where the rule applies afresh.

*What the log says.* `transition` stays a pure function: the decision is `verify::is_an_answer` over the job, called
from `completed` before a verification is started. The thread goes from `working` straight to `done`: one `thread_state`
event `done`, the `agent_status` `completed`, **no `check_result`, no `rework`, no `error`**. A `check_result` would have
to say `passed`, `failed` or `pending` about work that does not exist, and the UI would draw a card for it; the honest
record of "nothing was pushed, so there was nothing to verify" is the absence of a verdict (the web shows no
**Checking the work…** pill and no check steps, only the agent's answer). `Job.verification` is not counted either: no
verification started. The AG-UI projection says the same: it used to send a `STATE_SNAPSHOT` `verifying` at every `completed`
under a gate, and it now asks the one rule the core asks (`orch_core::is_answer(pushed, branch_refused, attempt)`, from the
`branch` artifacts it has seen) before it does, so the stream goes from `SUBAGENT_FINISHED` to `STATE_SNAPSHOT` `done` and
`RUN_FINISHED` with no `verifying` and no card ([`api/agui.md`](../api/agui.md)). No event, field or wire name was added, so no
schema, golden or client changed.

*What replaces what.* This supersedes, for a first attempt that pushed nothing, the 2026-09-30 rule that "checks, but no
pushed commit" is a failed source and that the owner's "Hi" ends in a rework and then `Failed`
([the 2026-09-30 note](#status-note-2026-09-30-the-agents-checks-need-a-pushed-commit)). Everything else in that note
stands: checks count only on the pushed commit, the rework prompt carries the person's messages, the fence grammar, and
the list of what the gate still does not check. The "Hi" case that note was written for is `Done` on attempt 1 with the
agent's reply, which is what `gate.rs` of the app's tests now asserts (the test that said "checks with no pushed commit
never end the thread done" was rewritten, not deleted: it pinned the old rule).

*The cost, said plainly.* The gate can no longer tell an agent that forgot to push from an agent that answered. A coder
asked for a pull request that finishes without a `branch` artifact ends `Done` with no pull request, as a chat would. What
stands between that and the person is the agent's own answer (the coder says what it did), the chat, and the person's next
message, which starts the next job under the same gate. A deployment that wants "every job pushes" has no option for it:
that would be a new gate setting (a source or a flag), proposed on its own if the owner wants it.

*Threads in flight.* A thread that was reworked under the old rule is at attempt 2 or 3 with nothing pushed and keeps
failing on "no pushed commit" if its agent keeps pushing nothing; a thread that is `Verifying` is unaffected. New
jobs get the new rule.

*Verified 2026-10-04 by the core's and the app's tests:* `an_agent_that_pushed_nothing_and_reported_no_checks_gave_an_answer`,
`an_agent_that_pushed_nothing_and_whose_checks_passed_gave_an_answer`,
`an_agent_that_pushed_nothing_and_whose_checks_failed_gave_an_answer_too`,
`an_unreadable_checks_artifact_does_not_make_an_answer_a_failure`,
`a_branch_artifact_the_gate_cannot_use_is_a_failed_push_not_an_answer`, `a_push_is_verified_exactly_as_before`,
`a_rework_that_pushes_nothing_is_not_an_answer` (`crates/core/tests/gate.rs`), the property test's rule (4) and (4b)
(`gate_props.rs`: done only when every required source passed, except an answer, which has no verdict), the projection's
`an_agent_that_pushed_nothing_is_done_and_the_thread_is_never_said_to_be_verifying` and
`a_rework_that_pushed_nothing_is_still_said_to_be_verified` (`crates/agui-projection/tests/verify.rs`), and the app's
`an_agent_that_pushed_nothing_gave_an_answer_and_the_thread_is_done`,
`a_branch_artifact_that_is_unusable_fails_with_its_reason` and
`pushed_work_is_verified_and_a_rework_that_pushes_nothing_is_not_an_answer` (`crates/app/tests/gate.rs`).

## Status note (2026-10-07): pushed work is verified in every attempt, and a failure on the base is a note

*Why.* The owner's production exports of 2026-10-06 and 2026-10-07 show the gate failing work that was never pushed, and
failing work for a failure it did not cause:

* **A** ("Who's Christian Yemele?", a fork, job 4): the coder built and ran a script in a scratch project, its own checks
  passed, it pushed nothing and reported no `branch` artifact. The gate answered with "no pushed commit: the agent reported no
  `branch` artifact, so there is nothing to check", reworked it twice and ended the thread `Failed`. The OSINT thread did the
  same, then blocked asking for a repository.
* **B** ("What is this repo about?", job 2, "Can you investigate more?"): an investigation only; the gate said "no checks
  reported: no `checks` artifact came before the agent finished" and reworked it.
* **C** (the same thread, job 1): `yarn check` failed on errors the repository already had; three attempts were spent and no
  pull request came.

*What the code did, read 2026-10-07 (`verify.rs`, `gate.rs`, `transition.rs`).* `is_an_answer` was
`!pushed && !branch_refused && attempt <= 1`. **Every route into a gated attempt 1 needs a `pushed` ref or a `branch_problem`**:
a first attempt with no `branch` artifact at all is `Done` (the core's `an_agent_that_pushed_nothing_*` tests, and the app's,
pin it), and `Job::next` resets the attempt to 1 and forgets `pushed`, `results` and `branch_problem` for every new job, a fork's
included. So the finding of the exports ("no `branch` artifact", on every attempt) can only be what **attempt 2 and 3**
say: once a job is at attempt 2 the rule did not apply any more, and the question the agent could not answer (push what, in a
scratch project?) failed it again, twice, whatever it did. Two defects follow from the code, and are fixed here; **the first
trigger of A and B, an attempt 1 that was gated, cannot be reproduced from the core alone** and is *unverified*: it needs a
`branch` artifact in that attempt (a usable one, or one the gate refused), which the exports' artifact events would show.
Whoever reads them next should look for it (open point below).

1. *`attempt <= 1` was a proxy.* The rule's reason is "a rework exists because a push failed: the pushed branch is still the work".
   An attempt number says nothing about that: a rework that exists because the `branch` artifact **could not be used** (a short
   hash, a repository that is no address) has no pushed commit at all, and finishing it with nothing pushed is not leaving a red
   gate, it is an agent that, told why, has nothing to push. The core now keeps **the last commit an earlier attempt of the job
   pushed** (`Job.earlier_push`, set by the rework, absent on a stored ledger from before, forgotten by the next job) and
   `is_answer(pushed, branch_refused, earlier_push)` reads it instead of the attempt. A rework of a pushed commit keeps the old
   rule exactly (the pushed branch is still the work; "no pushed commit" fails it, now *naming* the commit, branch and repository
   that are still the work, so the agent can act on it). A `branch` artifact the gate cannot use is still a failed push in the
   attempt it is sent in (it is what an agent that tried and got it wrong looks like).
2. *"no checks reported" was said first.* `agent_checks` looked for the `checks` artifact before it looked at the push, so a job
   whose gate applied with nothing pushed was told to report checks for a tree nobody pushed (B's message). With no pushed
   commit the finding is now always the no-push reason (and what the failed checks said, after it); "no checks reported" is only
   for a **pushed** commit.

| What the job holds when the agent finishes | The gate (2026-10-07) |
|---|---|
| no usable `branch` artifact, none refused, no earlier push (**any attempt, any job**; whatever else was reported) | does not apply: `Done`, no verdict of any source |
| a `branch` artifact the gate could not use | applies: "the `branch` artifact was not usable: <reason>" |
| a usable `branch` artifact | applies as before |
| no new `branch` artifact in a rework of a commit an earlier attempt pushed | applies: "no pushed commit; an earlier attempt pushed commit <12 hex> to branch <b> of <repo>, which is still the work: push the fix there and report the new `branch` artifact" |

*Pre-existing failures pass with a note (C).* adam-rs re-runs a failing check on the base commit of the pushed work; when it
fails there too the coder marks the finding `preexisting: true` (with the base commit). The contract is in
[`api/agui.md`](../api/agui.md#the-agents-checks-artifact). The core reads each finding: a string, or an object whose `preexisting`
is the boolean `true`, is a pre-existing failure; everything else (a missing field, `"true"`, `1`) is the agent's to fix, so an older
agent behaves exactly as before. A failing report whose findings are **all** marked passes: `check_result` is `passed`, with no
findings, and its `summary` is the agent's own followed by "failing on the base commit <12 hex> too, so not caused by this
work: `yarn check`, ..." (the base commit when a finding or the report names a full hash). A report that fails on something
unmarked fails, its findings the unmarked ones followed by one that names the marked ones "leave those": the agent is not sent
to fix what it did not break. The pre-existing note is a claim of the agent's, as its checks are: it is not a verdict of CI or of
the verifier, which, when required, still judge the commit themselves (the note is the cost, said plainly: an agent that marks
a check it broke gets through `agent-checks`; the gate trusts the mark as it trusts `passed`, and the base commit named in the
summary lets a reviewer re-run it).

*What stays.* The rework prompt, the fences, the attempt budget, CI and the verifier, checks counting only on the pushed commit, the
unusable `branch` as a failed push. No event, field of an event or wire name changed: `Job.earlier_push` is a ledger field
(omitted when absent, so a ledger stored before it reads as it did; a thread in flight at attempt 2 or 3 with nothing pushed
before is now an answer when it finishes without pushing) and `ChecksReport` gained `preexisting` and `base_commit`
(the core's own type, not a wire type). `orch_core::is_answer` changed its third argument from the attempt to
"an earlier attempt pushed a commit": a host that calls it (the AG-UI projection is the one in this repository) must pass
that, and the projection tracks it as the core does, from the `rework` events.

*Verified 2026-10-07 by tests:* the core's `a_job_that_pushed_nothing_is_done_in_every_attempt_whatever_checks_came`,
`a_job_after_a_failed_job_is_gated_afresh_and_an_answer_is_still_an_answer`,
`a_rework_after_an_unusable_branch_that_pushes_nothing_is_an_answer`,
`a_rework_of_a_pushed_commit_keeps_it_as_the_work_and_names_it`,
`checks_that_fail_only_on_checks_the_base_fails_too_pass_with_a_note`,
`a_failure_that_is_new_still_fails_and_the_base_failures_are_named_after_it`,
`what_the_gate_does_not_understand_as_preexisting_is_not` and
`pre_existing_checks_on_another_commit_than_the_pushed_one_still_fail` (`crates/core/tests/gate.rs`); the property test's
answer rule now reads `earlier_push` (`gate_props.rs`); the app's `an_investigation_after_a_failed_job_is_an_answer_not_a_failure`,
`a_rework_after_an_unusable_branch_that_pushes_nothing_is_done` and
`checks_that_fail_on_the_base_too_pass_and_the_result_says_which` (`crates/app/tests/gate.rs`). *Unverified:* that adam-rs's coder
writes `preexisting` and `base_commit` as described (its change is a separate pull request), and what the exports' first
attempt held.

## Configuration summary

| Variable | Default | Meaning |
|---|---|---|
| `ORCH_GATE` | empty | required sources, comma list of `ci`, `agent-checks`, `verifier`; empty keeps today's behaviour. *Built: all three are accepted (`ci` since slice 6, `verifier` since slice 10)* |
| `ORCH_MAX_ATTEMPTS` | `3` | attempts per job, including the first |
| `ORCH_MAX_ATTEMPTS_CAP` | `10` | the most a target or a thread may raise it to; at most `100`. A default `ORCH_MAX_ATTEMPTS` is lowered to a smaller cap |
| `ORCH_VERIFIER` | none | the verifier agent's id (must be configured and, when the gate requires the verifier, another agent than the one verified; startup error otherwise). *Built (slice 10)* |
| `ORCH_VERIFIER_TIMEOUT_SECS` | `1800` | how long to wait for a verdict before `Blocked`, at least 1. *Built (slice 10)* |
| `ORCH_VERIFIER_WATCH_SECS` | `5` | how often a verification looks at its thread while it waits for the verifier, at least 1; not a policy of the gate, a setting of the dispatcher. *Built (slice 10)* |
| `ORCH_CI_TIMEOUT_SECS` | `3600` | how long to wait for CI before `Blocked` (ADR 0017), at least 1; an `AGENTS_FILE` entry's `gate.ci.timeoutSecs` overrides it. *Read since slice 6* |
| `ORCH_CI_REQUIRED` | none | the names of the CI checks that must pass, comma list; a gate that requires `ci` must name at least one here or in an entry's `gate.ci.required` (exit 78 otherwise). *Read since the review of 2026-09-30* |
| `AGENTS_FILE` `gate` | none | per-target override, shape above |

## Security notes

- **Findings are untrusted data.** They come from CI, an agent or a verifier and are put in a
  prompt for the worker. They are quoted and delimited, capped (20 items, 16 KiB per source), and
  never interpreted as instructions to the orchestrator.
- **The gate cannot be weakened from below.** A thread's `forwardedProps` or `start_job.gate` can
  only add sources and lower or raise attempts within the cap; a client cannot remove a required
  source, so a hostile or careless client cannot turn a gated target into an ungated one.
- **The verifier cannot complete the job.** Its envelopes never become `Input::Agent`; only a
  `verdict` artifact does, as a single `VerifierReported`.
- **Fail closed.** No verdict, no checks reported, no pushed commit after a push was attempted: each is a failed
  check. The agent's own checks are refused as well when they name another commit or none (status note of 2026-09-30,
  the first live run). Work that was never pushed is not work the gate has an opinion on (status note of 2026-10-04):
  the job is an answer, never a pass of any source, and no `check_result` says otherwise.
  Timeouts block; they never pass.
- The attempt cap bounds token and wall-clock spend by construction; the wait timeouts bound the
  rest.

## Consequences

**Easier**

- "Never done while red" is a property the tests can state: never `Done` while a required source is
  failed or pending; attempts never exceed the maximum; terminal states absorb; replay is
  deterministic.
- A deployment picks its trust: CI only, or CI plus a verifier, or the agent's own checks for inner
  loops; nothing else in the design changes.
- The existing behaviour and goldens are unchanged while `ORCH_GATE` is empty.

**Harder**

- `AgentChecks` needs an adam-rs change (a `checks` artifact from `run_checks`). Until then it is
  unusable with the default agent.
- The run now stays open longer, so a client sees `RUN_FINISHED` later than the agent's
  `completed`; every AG-UI consumer must tolerate it (the reference client does; the web has since slice 4).
- Three configuration layers to explain and test. The monotonic rule (may add, may not remove) is
  the simplification.
- A `Blocked` thread from a timeout needs a human; there is no automatic retry of the wait.
- **A chat is not verified, and the gate cannot tell a forgotten push from an answer.** Since 2026-10-04 an agent that
  finishes its first attempt with no `branch` artifact is done, whatever it reported. An agent that was asked for a pull
  request and forgot to push ends `Done` with nothing, as an answer would; only a job that pushed, or tried to, is
  held to the gate ([status note](#status-note-2026-10-04-only-pushed-work-is-verified)).
- **The gate does not see everything.** `agent-checks` is the agent vouching for itself; the orchestrator does not
  compare `pushed.repository` with what the person asked for and does not refuse a `commit` equal to the base; only `ci`
  and the verifier look at the commit independently. The holes are listed, with what stands in for each, in the
  [status note of 2026-09-30](#status-note-2026-09-30-the-agents-checks-need-a-pushed-commit) ("What the gate still does
  not check").

## Alternatives considered

- **CI only** (the answer proposed with open question 8). Rejected as the only option: repositories
  without CI, and inner loops, need something faster; it stays one of the three sources.
- **A verifier agent only.** Rejected: it can diverge from real CI ([ADR 0002](0002-verification-over-consensus.md):
  an external judge). It is optional here and never replaces a required CI.
- **A port for "checks"** (a `Verifier` trait). Rejected: the sources are a closed set, and a port
  would move the aggregation out of the pure core. The verifier is an ordinary A2A agent behind the
  existing `AgentClient` port.
- **A `Reworking` state.** Rejected: a rework is a delegation with `attempt > 1`, already
  representable as `Queued` or `Working`.
- **Spending an attempt on a timeout.** Rejected: a missing CI report is not evidence the code is
  bad, and burning attempts on infrastructure faults hides them. A timeout blocks and asks.
- **Letting a thread remove a required source.** Rejected: it would let any client bypass a
  target's gate.
- **Unbounded rework.** Rejected by ADR 0002.

## Hard to reverse

- **The `CheckSource`, `Timer`, `Input` (`VerifierFailed` included) and `EventBody` variants** and their
  serde names in `threads.job` and the event log.
- **Outbox kind `verify` and `outbox.task_id`.**
- **The `vymalo.check` and `vymalo.rework` activity schemas** once the web and other clients read them.
- **The `verdict` and `checks` artifact shapes**, which are contracts with other repositories
  (adam-coder, verifier agents).
- **`AGENTS_FILE` `gate`** in deployments' manifests.

Easy to reverse: every default, the caps and timeouts.

## Verified

- *Verified 2026-09-30* (adam-rs `882e239`): adam-coder's artifacts are `branch` and
  `pull_request`; `run_checks` emits none. The coder image used by the dev stack is
  `ghcr.io/vymalo/another-adam-rs/coder:sha-882e239@sha256:abd9233d45aef86784c8f986484680d287bf2ee12d5d81f111d4e7030a1e0cb7`.
- *Verified 2026-09-30* (this repository at `a35fa57`): the thread states are `queued`, `working`,
  `blocked`, `done`, `failed`, `cancelled`; the outbox kinds are `delegate` and `cancel`.
- AG-UI facts (`STATE_SNAPSHOT`, `SUBAGENT_*`, `RUN_ERROR`, activity messages) are as
  [`api/agui.md`](../api/agui.md) records them, *verified 2026-09-29* there.
- *Verified 2026-09-30* (this repository, slice 10, against WireMock 3.13.2 standing in for a verifier, and the real
  binary against it): the request the dispatcher sends a verifier is an A2A `SendStreamingMessage` with the
  verification context as `contextId`, the outbox row id as `messageId` and the prompt in `parts[0].text`; the
  `dev/wiremock/verifier` mappings match exactly that shape (`dev/check-mocks.sh`, `dev/verifier-e2e.sh`).
- *Cross-repository work:* adam-coder must emit `checks {passed, commit, summary, findings}` from
  `run_checks` (an adam-rs pull request). Not started.
