# orch-core

The pure core of the orchestrator: the contract types of the chat API, the job
ledger and the verification gate, and the one function that decides what happens
next, `transition`. No async, no I/O, no protocol dependencies.

## Where it sits

The innermost crate. Dependency direction: `core` <- `ports` <- `app` <- `api`
([workspace overview](../../README.md#crates)). Nothing here talks to a store,
an agent or the network, so the compiler enforces that the core stays pure
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md),
[ADR 0004](../../../docs/decisions/0004-closed-enums-over-dyn-registry.md):
protocols are closed enums). The JSON shapes match
[`docs/api/chat-api.yaml`](../../../docs/api/chat-api.yaml); the design is in
[`docs/orchestrator.md`](../../../docs/orchestrator.md).

## API at a glance

| Item | What |
|---|---|
| `transition(&Snapshot, &Input) -> Result<(Snapshot, Vec<Command>), TransitionError>` | the pure decision function |
| `Snapshot { state, job }`, `Snapshot::new(state)`, `Snapshot::queued(gate)`, `ThreadRecord::snapshot()` | what `transition` takes and returns: the thread state and its job ledger |
| `ThreadState` | `Queued`, `Working`, `Verifying`, `Blocked`, `Done`, `Failed`, `Cancelled`. `Verifying` (the agent finished and the gate is checking) is only ever entered under a gate that requires something, and is implied by the `check_result` events, never announced by a `thread_state` event |
| `Input` | `UserMessage { user, text, message_id?, run_id?, origin }` (the ids a surface such as AG-UI names, and the `Origin` of the surface, are recorded in the log), `UiAction { user, action }` (the user acted on an A2UI surface; answers a blocked thread like a message; refused on a finished job), `Redeliver { text }` (the dispatcher's: a user message already in the log whose delegation never reached the agent; starts the next job on a `done` or `failed` thread, nothing on a `cancelled` one, delegates on an open one), `Cancel`, `Agent { agent, revision, update }`, `DeliveryFailed { reason, retryable }`, `CancelledBeforeStart`, `CancelRejected`, and the machine inputs `CiReported(CiReport)`, `VerifierReported { attempt, verification, verdict }`, `VerifierFailed { attempt, verification, reason }` (the verifier cannot be used: the current verification waiting for it holds the thread, `blocked`, no attempt spent; any other changes nothing) and `TimerFired(Timer)` (a surface must never let a user submit those) |
| `Command` | `Append(EventDraft)`, `Delegate { text }`, `DelegateAction { action }`, `RequestCancel { job }` (the job the person asked to stop), and for the gate `Watch { key }`, `Schedule { after, timer }`, `RequestVerification { attempt, verification, verifier, pushed, text }` (the application executes all three: `Watch` and `Schedule` since slice 5, `RequestVerification` as an outbox row of kind `verify` since slice 10) |
| `JobView`, `Job::view()`, `CheckSource::ALL`, `CheckSource::config_name` / `from_config_name` | what clients are told about a job (`Thread.job` and the AG-UI `STATE_SNAPSHOT` `job`: `{number?, attempt, maxAttempts, gate, sha?}`, `number` only from job 2, only under an active gate; `ThreadRecord` serialises it), and the configuration spelling of a source (`ci`, `agent-checks`, `verifier`) |
| `Job` (`number`, from 1, and `Job::next()`: the next job of a thread, ADR 0020), `JobStartedData` (the `job_started` event), `GatePolicy`, `CiPolicy`, `CheckSource`, `CheckResult`, `CheckStatus`, `PushedRef`, `Hold`, `Timer`, `WatchKey`, `Verdict` | the job ledger ([ADR 0016](../../../docs/decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md)) and the gate ([ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md)). `Job::default()` is a job with no gate: the core never touches it and behaves as it did before the gate existed. A gate that requires nothing (`GatePolicy::is_active() == false`) is that default. The policy is copied into the job at creation |
| `CiReport`, `CiConclusion`, `CiProvider` | a completed CI check, normalised by a surface. `success`, `neutral` and `skipped` pass; the rest of the closed enum fails |
| `recognise_artifact(name, text) -> Recognised`, `repo_key(url)`, `is_commit_hash`, `cap_findings` | pure helpers: the two artifacts the gate reads (`branch {repository, branch, commit}` and `checks {passed, commit, summary?, findings?}`), the `host/owner/name` repository key (lower case, no `.git`), and the 20-item, 16 KiB bound on findings. `MAX_FINDINGS`, `MAX_FINDINGS_BYTES` |
| `Origin` | the surface a user message came in through: `Agui` (the default; the log does not spell it, so an event without `origin` reads as it) or `Mcp` (ADR 0019); on `UserMessageData` and `Input::UserMessage` |
| `parse_verdict(text) -> Result<Verdict, String>`, `is_branch_name(name)`, `MAX_BRANCH_BYTES`, `Verdict::missing()`, `Verdict::unusable(why)`, `verifier_context(thread, attempt, verification)`, `GatePolicy::set_verifier_timeout_secs` | the verifier's side of the gate (ADR 0018, slice 10): the `verdict {passed, findings[]}` artifact read strictly (a verifier is untrusted: `passed` must be a boolean, `findings` a list, and they are capped like every finding), the failed verdicts a verifier that says nothing usable gets (both say "no verdict"), the A2A context a verification runs in (`<thread>-verify-<attempt>-<verification>`, never the worker's) and the wait for the verifier in whole seconds. `Job.task` keeps the person's messages of the job in order (`[next message]` between them, at most `MAX_TASK_BYTES`: the first message and the newest ones, `[… earlier messages omitted …]` for the middle, ` [cut]` on a cut message), under every active gate, for the rework and the verifier's prompts; `Job.branch_problem` keeps why the agent's last `branch` artifact was refused, for the finding of a source with no pushed commit. `Job.summary` keeps what the worker said about its work (at most `MAX_SUMMARY_BYTES`, only under a gate that requires the verifier) for the verifier's prompt, which the core writes: the commit (a hash), the attempt, and, quoted as untrusted data, where the worker says it pushed (repository and branch), the task and the summary. A `branch` artifact whose branch `git check-ref-format --branch` would refuse (`is_branch_name`) is malformed and pushes nothing |
| `Event`, `EventKind`, `EventBody`, `Actor`, `ActorType` and the `*Data` payloads | the append-only event log the chat renders; the gate adds `ci_result` (`CiReport`), `check_result` (`CheckResult`) and `rework` (`ReworkData`) |
| `AgentUpdate`, `AgentTaskState` | the protocol-neutral update an agent adapter produces: status, artifact, message, and the A2UI pair `Ui { operations }` (a payload that passed the envelope check) and `UiRejected { reason }` (one that did not: an `error` event, nothing of it passed on) |
| `check_operations`, `inspect`, `UiRejection`, `UiVersion`, `SurfaceOp`, `UiSurfaceData`, `UiActionData`, `UiActionError`, the `MAX_*` limits, `A2UI_EXTENSION_V0_9_1`, `A2UI_EXTENSION_V1_0`, `A2UI_MEDIA_TYPE` | A2UI in the core ([ADR 0013](../../../docs/decisions/0013-a2ui-generative-ui.md)): the envelope every agent payload passes (the A2A adapter checks each part, and `transition` checks every `AgentUpdate::Ui` again, turning an unchecked one into an `error` event; a JSON array of at most 256 messages and 64 KiB, each with a known `version` and exactly one of the four surface operations naming a `surfaceId`), the shape and size rules of a user's action, the extension URIs and the catalog ids of the web renderer. Pure functions over `serde_json::Value`; components are not validated here |
| `ThreadRecord`, `AgentInfo`, `AgentTarget`, `Releases` | thread and agent descriptions (`ThreadRecord.job` is never serialised to the contract `Thread`) |
| `AgentId`, `ThreadId`, `UserId`, `Timestamp` | ids and time (`jiff`, no `f64` time) |
| `Classify`, `ErrorClass`, `BoxError`, `report` | one classification model for every error: retry, HTTP status and exit-code decisions match on `ErrorClass` (`Transient`, `RateLimited`, `Conflict`, `Invalid`, `NotFound`, `Rejected`), never on variants |

```rust
use orch_core::{transition, Input, Origin, Snapshot, ThreadState, UserId};

// A follow-up in a blocked thread re-queues it and appends the user's message.
let (next, commands) = transition(
    &Snapshot::new(ThreadState::Blocked),
    &Input::UserMessage {
        user: UserId::new("me@example.com"),
        text: "main".into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
    },
)?;
assert_eq!(next.state, ThreadState::Queued);
```

## Features and environment

None.

## Tests

Offline, no environment variables.

* `tests/transition_table.rs`: one test per row of the transition table
  ([`docs/orchestrator.md`](../../../docs/orchestrator.md)), with the gate off.
* `tests/properties.rs`: `proptest` properties over random input sequences, with the gate off.
* `tests/wire.rs` also pins `Thread.job` (present only under a gate, and nothing else of the ledger with it) and the wire shapes of `check_result` and `rework`.
* `tests/gate.rs`: the verification gate, one test per rule of the loop: the artifacts, the agent-checks
  source (which passes only with checks that name the pushed commit: no pushed commit, checks that name no commit
  and checks for another commit each fail; ADR 0018, status note of 2026-09-30), the rework prompt (the person's messages, all of them in order,
  in their own fence before the findings, capped with the first and the newest kept, unable to close its fence; the task kept under every active gate), the `branch` artifact the gate refuses and why, CI (current, stale, early, required names), the verifier, the deadlines, an abandoned verification
  and the `verification` counter, rework and running out of attempts, findings caps and quoting, repository
  keys, and the stored shape of `Job` and the new events; and a thread as a conversation (ADR 0020): a message on a finished thread starts job *n+1*
  (the gate kept, the attempt back to 1, the verification count kept, so a timer, verdict or CI report of an earlier job is stale), a redelivery, and a cancel that names its job.
  The verifier's rules: the request (commit, attempt, quoted pushed ref, task and summary, and nothing of the worker's outside a fence), the branch names git refuses, a verdict and a failure of the current or another verification, the hold and what answering it does, `parse_verdict` and its caps, the context of each verification.
* `tests/gate_props.rs`: `proptest` properties of the gate: never `done` while a required source is failed
  or pending (read from the events alone), `1 <= attempt <= max`, terminal states absorb, an input for another
  attempt or verification never changes state or job, a job survives JSON, replay is deterministic, and
  with no gate the job is never touched.
* `tests/wire.rs`: the JSON must match the contract schemas exactly, including the additive kinds `ui_surface` and `ui_action`.
* `src/ui.rs` (unit): the envelope rules one by one, the caps at their edge, that a refusal repeats at most an excerpt, the action checks.

## See also

[`orch-ports`](../ports/README.md), [`orch-app`](../app/README.md).
