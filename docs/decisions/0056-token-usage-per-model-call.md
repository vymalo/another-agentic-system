# ADR 0056 — Token usage per model call: logged once, projected to AG-UI, drawn as a ring beside the composer

- **Status:** accepted (2026-10-08), at the owner's request: "a token gauge (LibreChat-like ring): usage per model call from
  adam-rs, to the orchestrator's log, to the web; sub-agents counted apart". The contract is
  [`usage-v1.md`](../api/usage-v1.md), the optional-extension pattern [ADR 0008](0008-platform-integration-via-a2a-extension.md),
  the step paths [ADR 0025](0025-nested-steps-events-carry-their-source-path.md), the asked agents
  [ADR 0026](0026-agent-mentions-as-structured-references.md). The agent's side is adam-rs ADR 0032, built in parallel against the
  same contract and merged as adam-rs `09291a6` (PR #99), pinned here since 2026-10-09. It adds two kinds to the log, the reason for migration 0020.
  **Built 2026-10-09** on the orchestrator's and the web's side, proven on mocks (the fake A2A agent, the goldens, the web's mock
  server, a WireMock agent: `dev/usage-e2e.sh`).
  **Amended 2026-10-27:** the totals object gains an optional `sandbox` member, the time a run's commands used in a pod
  ([`usage-v1.md`](../api/usage-v1.md) section 5; adam-rs `55bbe9f`, its ADR 0043, issue 126). Decisions 2 to 4 and 6 are unchanged; the
  amendment is the section "Amendment 2026-10-27: the sandbox member" at the end.

## Context

The owner wants to see how full the model's context is while an agent works, and what a thread spent, with what each sub-agent
spent counted apart. Nothing carried token counts: the agent's model client read them and dropped them, A2A has no member for them,
the log had no kind for them and the web had no place to draw them.

Facts the design rests on:

* **AG-UI 1.0 has an accounting for tokens**, `TokenUsage` (`provider`, `model`, the totals `inputTokens`, `outputTokens`,
  `totalTokens`, and the parts `reasoningTokens`, `cachedInputTokens`, `cacheWriteInputTokens`, each an integer up to 2^53 - 1), on
  `RUN_FINISHED.usage` and `RUN_ERROR.usage`, one entry per provider and model. "The run is the accounting boundary: usage covers
  every model call made within the run, calls made by its subagents included; an agent invoked as a separate run under parentRunId
  reports its own usage on its own terminal event; and a run that resumes an interrupted one reports only the calls it made itself."
  *Verified 2026-10-08* in the vendored `orchestrator/crates/agui-proto/schema/ag-ui-1.0.schema.json` (`$defs.TokenUsage`,
  `RunFinishedEvent.usage`, `RunErrorEvent.usage`). `SUBAGENT_FINISHED` has no `usage`; a `CUSTOM` event may carry a
  `subagentRunId` (same file, `CustomEvent` is `Attributable`).
* **A2A carries extension data in `metadata` maps.** `TaskStatusUpdateEvent` has a `metadata` map beside `status`, so a report needs
  no message (*verified 2026-10-08* in `a2a-lf` 0.3.1, `src/event.rs`). A **streaming** client never sees the `Task`'s own `metadata`
  unless the server sends a `Task` frame, and adam's server ends its stream at the first terminal or interrupted status with no
  final `Task` (*verified 2026-10-08* in adam-rs `crates/adam-a2a/src/handler.rs`, `a2a_stream`).
* **Numbers in A2A metadata come back as doubles**: the metadata is a protobuf `Struct`, so `41250` arrives as `41250.0`, on
  JSON-RPC and REST alike (the reason `text-stream/v1` reads an offset as a float as well,
  `orchestrator/crates/a2a-mapping/src/lib.rs`, `whole_number`; the adam-rs side of `usage/v1` sees the same, reported by its
  builder on 2026-10-09, *unverified* here).
* **adam keeps the totals on the task only**: `Task.metadata[URI].totals`; the stream's last status update carries none
  (reported by the adam-rs side's builder on 2026-10-09, *unverified* here).

## Decision

1. **The extension.** `usage/v1` ([`usage-v1.md`](../api/usage-v1.md)) is optional and fails closed (invariant 2,
   [ADR 0008](0008-platform-integration-via-a2a-extension.md)): `KnownExtension::Usage`, read from the live card on every call, never
   cached, activated (the `A2A-Extensions` header and `message.extensions`) on `SendStreamingMessage` and `SubscribeToTask` only when
   the card lists the exact URI, exactly like `steps/v1`. An agent that does not list it is sent nothing new, reports nothing, and
   the screen shows nothing for it. Removing it breaks no plain A2A agent.
2. **The adapter reads, the core decides.** `orch-a2a-mapping` reads a **call report** from the `metadata` of a `working`
   `TaskStatusUpdateEvent` and the **totals** only from the task's own `metadata` (a `Task` frame, a `GetTask` poll), where the
   contract keeps them: a status update carries none, and an entry on one is not read. When a stream that activated the extension
   reaches a status that ends or pauses the task, the adapter reads the task once (`GetTask`) and passes on its totals before the
   status: a streaming client sees no task metadata otherwise. A report is checked against the contract by `orch_core::usage` (required members, integers from 0 to
   2^53 - 1, a whole number written as a double accepted, `totalTokens` the sum, labels and the call id at most 128 bytes, at most 32
   totals, one per provider and model). What does not pass is `AgentUpdate::UsageRejected`: never logged, never a task failure,
   counted (`usage_reports_dropped_total{reason="invalid"}`). A status update that carries a report and **no message** is the report
   and nothing else: no empty message, no status line.
3. **Two events, labels and numbers only.**
   * `model_usage` `{job, agent, task, call, path, provider?, model, inputTokens, outputTokens, totalTokens, reasoningTokens?,
     cachedInputTokens?, cacheWriteInputTokens?, contextWindow?}`, one per valid call report, attributed to the agent.
   * `model_usage_total` `{job, agent, task, path?, totals: [TokenUsage]}`, when a task ended or paused.

   **Attribution** (`path`): a call under a `stepId` the job's step ledger holds open has the path a child step of that step would
   have (the step's own path and the step, the 8 nearest; the rule of `agent_step`); a `stepId` the ledger does not hold (never
   reported, or ended already) is the agent's own call, path empty. **An asked agent's** reports and totals are read on its ask's
   own stream (`Input::AskUsage`), attributed to the asked agent, with the path `["ask-<n>"]` (the path the asked agent's relayed steps
   carry), on the totals too. **Dedup** is the idempotency key the dispatcher already stores, `a2a:<task>:usage:<call>`, so a
   resubscribe that replays a call, a poll and another replica collapse into one event. **Bound:** a job logs at most
   **10 000** call reports (`MAX_USAGE_CALLS_PER_JOB`); past it a report is dropped and counted in the job ledger (`Job.usage`) and in
   `usage_reports_dropped_total{reason="job_limit"}`. Totals come at most once per turn of a task and are not capped. Postgres keeps
   events as JSON rows; migration 0020 only widens the `events.kind` check.
4. **The projection.** Each `model_usage` is `CUSTOM{name: "vymalo.usage"}` whose `value` is the event's data plus `by` (`{kind:
   "agent" | "subagent" | "ask", name}`: who spent it, read from the path as the projector stands) and `at`, attributed to the subagent
   the path names while it is open (a sub-agent step's `sub-step-<seq>`, an ask's `sub-ask-<n>`), else the agent's invocation. Each
   `model_usage_total` is `CUSTOM{name: "vymalo.usage_total"}`, attributed the same way: so **an asked agent's own usage is said under
   its sub-run**. `RUN_FINISHED.usage` and `RUN_ERROR.usage` are AG-UI's run accounting of the thread's agent's tasks (an asked
   agent's are said under its sub-run, as the contract's own row for asks says): for each such task the run touched, what is known of
   it (its latest totals, plus the calls reported after them; without totals, the sum of its calls, sub-agent steps included) less
   what an earlier run of it already said, summed per provider and model; absent when the run touched none. One run per task, which
   is the common case, says the task's latest totals as they are; a run that resumes a task after a question, or goes on after a
   message sent while the agent worked, says only what it added, as AG-UI requires. Usage opens no run of its own: on a thread that
   waits or is finished the event is folded and nothing is said. A replay folds the same events and says the same frames.
5. **The ring.** Beside the send button, in the composer's bottom row, where LibreChat puts its context ring. It fills with how full
   the context of the **latest call of the thread's agent** is (`inputTokens / contextWindow` of the last `vymalo.usage` whose `by.kind`
   is `agent`): neutral below 80 %, amber from 80 %, red from 95 %. A call with no `contextWindow` leaves a neutral ring with no fill,
   which still opens the details. Pressing it (a button, keyboard included) opens a popover with the thread's totals per model (input,
   output, reasoning, cached: each task's latest totals plus the calls after them, else its calls) and the calls of the agent, of each
   sub-agent and of each asked agent apart, by name, as text. A thread with no usage at all shows no ring. The web folds the two
   `CUSTOM` events itself (`features/chat/lib/usage.ts`), so a replay and the live stream give the same ring.
6. **What is not counted.** The orchestrator's own model calls (thread titles, descriptions); agents that do not list the
   extension; the calls the verifier makes (its stream is read for its verdict only); and the calls of OpenCode inside the coder,
   which adam-rs cannot see (they run in OpenCode's own process, behind ACP).

## Consequences

* The log gains two kinds; an older build cannot decode a log that has them. **Roll this build out on every replica before an agent
  that lists `usage/v1`**: nothing writes either event until such an agent does (the pinned adam lists it since adam-rs `09291a6`, 2026-10-09;
  an orchestrator that predates this change does not activate the extension, so an agent that lists it reports nothing to it).
* One extra `GetTask` per turn for an agent that lists the extension and leaves the totals out of the status that ends its turn.
* The export carries both events and `Job.usage`: labels and numbers, the task and call ids and the step path, no prompt or
  completion. A reader of a shared thread is sent the same frames, but the ring is the composer's, so a read-only view draws none.
* A fork copies the log, its usage included: a fork's ring starts where the parent's stood.
* A call report that arrives after its sub-agent step ended is counted as the agent's own (the step ledger forgets an ended step);
  adam reports a call before the step that holds it ends.

## Not done

* **The agent's totals can be lower than the sum of its call reports.** adam-rs ADR 0032 drops from the totals what it cannot keep:
  the state of a transient-retry attempt, a turn a cancel ended, a child canceled before it answered, remote `a2a:` sub-agents and
  OpenCode. The ring's details and `RUN_FINISHED.usage` follow the totals once a task has them (the record, as the contract says); the
  call reports stay for the live ring and for counting sub-agents apart, so the per-agent lines can add up to more than the totals.
  Nothing here reconciles the two.
* **adam's ids**: a call is `<run>-c<turn>-<8 hex>` and its `stepId` the root's `tool:<call id>`, so a grandchild's call is counted
  under the root's sub-agent step, not under the grandchild (adam-rs ADR 0032; *verified 2026-10-09* by reading it at `09291a6`, decisions 3
  and 4).
* ~~**The coder does not report yet, and the context window in compose.**~~ *Done 2026-10-09* (the follow-up on top of this change):
  the pin is adam-rs `09291a6` ([ADR 0014](0014-adam-coder-default-agent-over-a2a.md), its note of that day), whose cards list `usage/v1`,
  and `compose.yaml` sets `MODEL_CONTEXT_WINDOW=131072` for the coder and the folder agents, as adam's own compose does. `dev/greeting-e2e.sh`
  asserts the coder's greeting call (a `model_usage` with the window 131072) and its `model_usage_total` with the real image (Coder E2E
  only, not run where this was written). `compose.live.yaml` and the chart set no window: a real model's is the owner's, and without one the
  ring shows the totals but no fill.
* No live model was used, and no adam agent that lists the extension where this was written: everything is proven on the fake A2A agent,
  the goldens, the web's mock server and a WireMock agent; the real coder's greeting is left to Coder E2E (above).

## Alternatives rejected

* **Summing the call reports for the record.** A dropped stream loses reports; the agent's totals do not. The reports are for the live
  ring and for counting sub-agents apart, as the contract says.
* **Dedup in the job ledger.** A set of up to 10 000 call ids in `threads.job`, rewritten with every event; the idempotency keys already
  do it, durably and across replicas.
* **`RUN_FINISHED.usage` as the task's latest totals, whatever the run.** A task that spans runs (a question answered, a message sent
  while it works) would be counted once per run by any client that sums runs; AG-UI's rule is the delta.
* **The ring in the top bar beside the state pill.** The pill says what the thread does; LibreChat's users find the ring by the input,
  where the next call's context is written.

## Verified and unverified (2026-10-08)

* *Verified*: the AG-UI facts above (vendored schema); `TaskStatusUpdateEvent.metadata` (`a2a-lf` 0.3.1); adam's stream ends at the
  first status that ends or pauses the task.
* *Unverified* here (reported by the adam-rs side's builder on 2026-10-09, merged as `09291a6`, not read in this repository): totals on
  the task only, written on `completed`, `failed`, `canceled`, `input-required` and `auth-required`; numbers as whole doubles; call ids
  `<run>-c<turn>-<8 hex>`; `stepId` the root step `tool:<call id>`; `provider` `openai`; `contextWindow` only with
  `MODEL_CONTEXT_WINDOW`. Which gateways send cached and reasoning token counts. No live model was used.
* *Verified 2026-10-09* by reading adam-rs at `09291a6`, the pin since that day (not by running it): call ids `<run>-c<turn>-<8 hex>`, a child's
  report under its root's `tool:<call id>` step, `provider` `openai` from the OpenAI-compatible client, and `contextWindow` only with
  `MODEL_CONTEXT_WINDOW`, for the alias `MODEL` names (adam-rs ADR 0032, decisions 2 to 4; `crates/adam-service/src/config.rs`). The totals are
  read from the run's state and said on a task that is terminal, `input-required` or `auth-required` (`crates/adam-a2a-runtime/src/convert.rs`,
  `task_from_view`, `settled`); the module comment of `crates/adam-a2a-runtime/src/usage.rs` leaves `auth-required` out of its list, which is that
  comment's slip, not the code's.

## Amendment 2026-10-27: the sandbox member

*Added after the fact, at the request of adam-rs [issue 126](https://github.com/vymalo/another-adam-rs/issues/126) (M2, usage and limits
per person); the agent's side is adam-rs `55bbe9f`, its ADR 0043 (seven decisions, agreed on the issue before building). Nothing above is
rewritten.*

### What changed on the wire

The adam coder writes a member `sandbox` in the totals object of `usage/v1`, beside `totals` (the same URI; the card is unchanged):
`{"environment": "kubernetes", "usage": [{"cpuMillis", "memoryBytes", "commands", "milliseconds"}]}` for a run whose commands were
measured in a pod; `"usage": []` for a pod asked for and no command run (a measured zero); `{"environment": "local"}` or `"devcontainer"`
with no `usage` for commands that were not measured (**not measured, never zero**); no `sandbox` for a run that asked for no environment.
`totals` is always there (`[]` for a run with no model call). The contract is [section 5 of `usage-v1.md`](../api/usage-v1.md#5-the-sandbox),
with the rule that a client **ignores unknown members** of the totals object and of its entries.

### Decisions

1. **`sandbox` is read apart from `totals`; an invalid one is dropped alone.** `UsageTotals::parse` reads the tokens as before and then
   `SandboxUsage::parse` reads the member. If the member breaks the contract (not an object, no `environment`, an `environment` that is not
   one of the three words, `usage` not an array or with more than 8 entries, an entry without `commands` or `milliseconds`, a count that is
   not a whole number from 0 to 2^53 - 1), the totals are still taken: the logged `model_usage_total` has no `sandbox`, the reason is kept
   in `UsageTotals::sandbox_rejected`, and the application counts it. *Why:* the tokens have been reported since 2026-10-09 and are what a
   limit or a price counts; a fault in a member added later must not make them vanish. Rejecting the whole object, as the token counts'
   own rule does for a bad token count, would have tied the old data to the newer member's correctness. The cost is that a broken
   sandbox is visible only through the counter and a debug line, not in the log; that is the same trade as for a rejected call report.
2. **The counter is a new reason of the existing one**, `usage_reports_dropped_total{reason="sandbox_invalid"}`, not a counter of its own and
   not `invalid`. It is the same family (usage data that was not logged), so one alert on the family catches it; it is not `invalid`, so an
   alert on broken *token* reports does not move for a newer member's fault, and `reason` says which. It is counted once the totals are
   logged, so a replay of the same totals counts nothing. The HELP text now says "or sandbox members left out of logged totals".
3. **An unknown `environment` word is an invalid sandbox**, not a kept word. `SandboxEnvironment` is a closed enum (ADR 0004): a consumer that
   branches on it needs the three, and a word the orchestrator cannot interpret is better counted than logged and shown. A new environment
   is a change to the contract page; until the orchestrator is built for it, the tokens are kept and the counter says the orchestrator is behind.
4. **Unknown members inside `sandbox` and its entries are ignored**, like the totals' own, so adam-rs can add one without a new version.
   `null` is absent. Two entries of one shape are not rejected (the sum of entries is what a reader wants either way), and `usage` is not
   tied to the environment word: what says "measured" is the presence of `usage`.
5. **The log.** `model_usage_total`'s `data` gains an optional `sandbox` (not written when absent, read as absent when missing), so every
   event written before still reads and **no migration is needed**: events are JSON rows. `UsageUpdate::Total`'s `UsageTotals` carries the
   parsed member, and the core checks it again where it records it (`SandboxUsage::check`: at most 8 entries, counts in range); one that fails
   there is left out and the totals are logged, whatever adapter built it. A rolling deploy is safe both ways: an older replica reads an event
   with `sandbox` (serde ignores the unknown member) and shows nothing of it.
6. **The projection carries it; AG-UI's run accounting does not.** `CUSTOM vymalo.usage_total` is built from the event's data, so its `value`
   carries `sandbox` with no projector change. **No golden shows it, on purpose**: the digest table (`tests/projection-digests.txt`, history rule 7)
   pins the frames of every golden, and a change to a line it already had raises `PROJECTION_VERSION`, which tells every client that stores frames
   (ADR 0060) to drop them; making the `usage` golden's totals carry a sandbox would have been that change, for a projection that moves no event
   already in a log. The test `a_totals_sandbox_rides_the_custom_and_is_no_part_of_the_runs_tokens` and the WireMock agent pin it instead.
   `RUN_FINISHED.usage` and `RUN_ERROR.usage` stay AG-UI's `TokenUsage` and say the tokens alone:
   the type has no member for a sandbox, and a run's delta is a rule about tokens. The web reads `totals` of the CUSTOM and ignores the rest
   (`parseTotal`), so no screen changes. A fork copies the log and so the event, as it copies the tokens.
7. **What is not done here.** No screen draws it; no limit or price reads it (the price per CPU-second belongs to the usage record, issue 281);
   a pod's idle time is not reported by the agent (adam-rs ADR 0043, decision 1), so nothing here can bill it.

### Facts, and how they are known

| Fact | State |
|---|---|
| The agent writes `sandbox` beside `totals` in the four shapes, entries at most 8 with integer counts, `totals` always there | *verified 2026-10-27* by reading adam-rs at `55bbe9f` (its ADR 0043 and `crates/adam-a2a-runtime/src/usage.rs`, `totals_metadata`), not by running it |
| The orchestrator before this change ignored a member beside `totals` and inside an entry | *verified 2026-10-27* by a test that passes on the code before it (`unknown_members_beside_the_totals_and_inside_their_entries_are_ignored`) |
| An invalid sandbox keeps the totals; the log, the AG-UI `vymalo.usage_total` and the counter carry or count it | tested on the fake A2A agent (`usage-sandbox`, `usage-sandbox-bad`), the memory agent and the projector; the goldens are unchanged (decision 6); `dev/usage-e2e.sh`, `dev/check-mocks.sh` and `crates/e2e/tests/wiremock_agent.rs` against the WireMock agent `mock-usage` are for CI (Compose, Coder E2E) |
| Postgres round-trips the event and reads an event written without the member | the case `model_usage_total` of `crates/store-postgres/tests/postgres.rs`, which skips without `ORCH_TEST_POSTGRES_URL` (CI runs it) |
| A real coder in a real run pod writes figures this reads | *unverified*: adam-rs ADR 0043 itself says its fakes run the commands as local processes |
