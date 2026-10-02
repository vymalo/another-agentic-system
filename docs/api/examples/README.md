# Golden event transcripts

What the real orchestrator emits, one file per scripted agent behaviour. They pin the *meaning*
of the events (kinds, `agent_status` spellings, failure shape, message finality), which
[`chat-api.yaml`](../chat-api.yaml) cannot: `EventData` is an untyped bag there. (No operation returns
these events any more: the legacy `listEvents` and `streamEvents` were removed on 2026-09-30. They are
the log itself, which the AG-UI streams below project.)

| File | Agent script (first word of the message) | Ends in |
|---|---|---|
| `echo.events.json` | `echo`: working, artifact with the PR link, completed | `done` |
| `ask.events.json` | `ask`, then the answer `main`: blocked, resumed on the same task | `done` |
| `cancel.events.json` | `slow`, then Cancel | `cancelled` |
| `fail.events.json` | `fail`: `agent_status: failed` with `detail`, no `error` event | `failed` |
| `talk.events.json` | `talk`: status text, one final `agent_message`, the artifact | `done` |
| `release.events.json` | `echo` with release `staging`: the revision on every agent actor | `done` |
| `verify-green.events.json` | `verify-red-once`, on the fake agent and under a gate that requires the agent's own checks (the test world gives `plain` `gate: {require: [agent-checks]}`, like an `AGENTS_FILE` entry): the checks fail, `check_result` and `rework`, the agent goes again in a new task, the checks pass (ADR 0018) | `done`, attempt 2 of 3 |
| `verify-red.events.json` | `verify-red`, the same gate: three attempts whose checks all fail, two `rework`s, then the `error` and `thread_state: failed` | `failed`, `checks_failed` |
| `verify-verifier-green.events.json` | `verify-reviewed`, under a gate that requires a **verifier agent** (the test world gives `plain` `gate: {require: [verifier], verifier: reviewer}`, and `reviewer` is a fake verifier): the worker pushes and finishes, the verifier's `verdict` has findings (`check_result` pending then failed, `rework`), the worker goes again in a new task, the verifier passes it (ADR 0018, slice 10) | `done`, attempt 2 of 3 |
| `verify-verifier-red.events.json` | `verify-reviewed`, the same gate, a verifier that rejects every commit: three attempts, two `rework`s, then the `error` and `thread_state: failed` | `failed`, `checks_failed` |
| `ci.events.json` | `verify-ci` on the fake agent (it pushes a `branch` artifact and leaves the checking to CI), under a gate that requires CI (`plain` has `gate: {require: [ci]}`), with the test playing the CI system through `App::receive` and the inbox worker: a red `ci/build` for commit `…01` (`ci_result`, `check_result` failed, `rework`), the agent goes again and pushes `…02`, a green `ci/build` for it (ADR 0017) | `done`, attempt 2 of 3 |
| `a2ui.events.json` | `ui`, on an agent whose card lists the A2UI extension: a surface in two artifacts (`ui_surface` twice), the question, then the user's action through the AG-UI run route (`ui_action`) and the answer | `done` |
| `followup.events.json` | `echo hi`, then the follow-up `echo now add tests` on the finished thread: a message on a `done` thread starts job 2 (`user_message`, `job_started`), a new task on the same context (ADR 0020) | `done`, job 2 |
| `followup-after-cancel.events.json` | `slow work`, Cancel, then the follow-up `echo never mind, do this`: a stopped thread is not closed either | `done`, job 2 |
| `catalog.events.json` | `echo hi` through the AG-UI run route with the screen's UI catalog, version 1, then `echo again` with version 2 and `echo once more` with version 1 again, each a job of the thread (ADR 0023, MVP slice 3): `ui_catalog` is the first event of the first two jobs (version 1, then 2) and the third writes none, because its digest is known; the log holds the consumer's message and run ids, which a route that carries a catalog has | `done`, job 3 |
| `steps.events.json` | `steps run the tests` on the fake agent with `steps/v1` in its card (ADR 0025, MVP slice 5): a sub-agent step `OpenCode`, a command `npm test` under it, called with `{command, cwd, env}` (its `input`, on the start, with the `NPM_TOKEN` the core redacted to `[redacted]`: ADR 0030), that fails with the detail `1 failed` and the error text as its `output` (on the end), the sub-agent's end, then the agent's words and `completed`. Step ids are `<task>/<agent's id>`; the task id is normalised to `T` | `done` |
| `steps-ask.events.json` | `steps-ask clean the build`: the same sub-agent with a command that is `waiting` when the agent asks (`input_required`); after the answer (`yes`) the command and the sub-agent end in the next run | `done` |
| `working.events.json` | `stream-words go` on the fake agent with `text-stream/v1` and `steps/v1` in its card (ADR 0031): the words before a tool call stated on a `working` status (`agent_message` with `purpose: working`), a command `npm test`, then the reply stated on `completed` (`agent_message` with `purpose: answer`; the status keeps its `detail`). The text chunks are live and are not in the log | `done` |
| `turn-output.events.json` | `turn-output go` on the fake agent with `thread-tools/v1`, `text-stream/v1` and `steps/v1` in its card and a grant minted by the adapter (ADR 0031, the amendment): a sentence before a tool call stated on a `working` status (`purpose: working`), a command `npm test`, the agent's call of the `turn_output` tool (`agent_message` with `purpose: answer, via: turn_output`, the id `out-<jti>-1`, by the agent), then its closing line stated on `completed`, which the core writes as working text (`agent_message` with `purpose: working`) ahead of the status that keeps it as its `detail`. The fake waits half a second before it calls the tool, so the transcript is the same on every run (the call and the A2A stream are two roads) | `done` |
| `title.events.json` | `slow work`, then a person renames the thread while it works (`PATCH /api/threads/{id}`: `thread_titled` with `source: user`), Cancel, and renames it again once it is cancelled: the title is the person's from the first rename on, and a rename of a finished thread is an event like any other (MVP slice 6) | `cancelled` |
| `fork.events.json` | the log of a **fork** (ADR 0029): `echo one` finished, `echo two` after it, then the second message edited into a branch (`POST /api/threads/{id}/fork {replace, text}`). The fork's log is the first turn copied (events 1-5, as the parent has them), `thread_forked` (`kind: edit`, `from: {threadId, seq: 5}`, the parent's title), the replacing message, `job_started` 2 and the agent's second turn | `done`, job 2 |
| `fork-blocked.events.json` | a thread that waits for an answer (`ask about branches`), forked as it is (`{after}`: the copy ends with the question and `thread_state: blocked`, `thread_forked` with `kind: fork`), then a message on the fork (`echo thanks`), which starts job 2: the question is not the fork's to answer | `done`, job 2 |

[`stream.feed.json`](stream.feed.json) is not a transcript of a run: it is a log **and live text** in the order one connection
heard them (an array of `{"event": …}` as above and `{"live": {agent, messageId, offset, text, end}}`), written by hand because the
pieces and the log travel on different channels and no run can pin their interleaving (ADR 0027, MVP slice 6): the user asks,
the agent works, three pieces of the reply arrive (`Fib`, `onacci `, `in Rust.`), the log says the whole message and the
status that repeats it, and the thread is `done`.

Ids and clocks are normalised: `threadId` is `<thread-id>` (the thread a fork was cut from, `data.from.threadId` of `thread_forked`, is `<parent-thread-id>`), `at` is `<timestamp>`, an agent
message's `messageId` is `<message-id>` and the task id in front of a step's id and path is `T`.

- **Producer:** `orchestrator/crates/e2e/tests/golden.rs` (`transcripts_match_docs_api_examples`)
  runs each script through the real application (the first message enters through `App`, so the log holds no
  consumer-chosen ids; the action of `a2ui` and the `verify-*` runs: the AG-UI run route), dispatcher and A2A adapter, and fails when a file
  differs. `orch-api`'s `tests/contract.rs` validates every event of every file against the contract's `Event` schema. After an intended change, regenerate and review the diff:
  `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test golden`.
- **Consumers:** `web/mock/golden.test.ts` drives every scenario through the mock server's AG-UI routes
  and requires the connect stream to be the golden `agui/<name>.agui.json` below, so the mock tells the
  same story, `a2ui` included (a surface, the question, and the action that answers it through
  `forwardedProps.a2uiAction`), the four `verify-*` scenarios (the gate, with the agent's own checks and with a
  verifier agent; the web renders their cards since MVP slice 4, and the mock plays the verifier as a subagent) and `ci` (the
  mock plays the CI reports too; the web renders the card since MVP slice 8). The web renders the AG-UI goldens, not these event logs: see the next section.

## AG-UI streams

[`agui/<name>.agui.json`](agui/) is what a **viewer** (an AG-UI connect stream, see
[`../agui.md`](../agui.md)) is shown for the same log: the frames `orch-agui-projection` produces
from each `<name>.events.json`, as an array of `{"id"?: <seq>, "event": <AG-UI event>}`. `id` is the
SSE `id:` a client resumes from, and is present only on the last frame of a log event with no text
message open. `threadId` is `<thread-id>` (a real thread id in any stream); the runs are
`run-<seq>`, the invocations `sub-<seq>`, and the interrupt `int-<seq>`.

- **Producer:** `orchestrator/crates/agui-projection/tests/golden.rs` projects the `*.events.json`
  files above (with placeholders made real, and `stream.feed.json` through the live overlay) and fails when a file differs.
  `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test golden` regenerates them; review the
  diff.
- **Consumers:** the same test checks every event against the vendored AG-UI schema and every
  stream against the well-formedness rules; `tools/agui-conformance` feeds the frames, as SSE, through
  the reference client (`@ag-ui/client` 1.0.0) in CI; `web/mock/golden.test.ts` requires the mock
  server to produce them, and `web/src/features/chat/lib/agui/runtime-goldens.dom.test.tsx` runs them
  (and the `connect-*` ones) through the runtime the chat surface uses, `@assistant-ui/react-ag-ui`
  with `web/patches` applied, and checks the transcript.

The `verify-green.agui.json` and `verify-red.agui.json` goldens are the verification gate a viewer reads (ADR 0018,
[`../agui.md`](../agui.md#verification-the-gate)): **one run** for all the attempts, `SUBAGENT_FINISHED` at each
`completed` and never `RUN_FINISHED` until the job is done or out of attempts, the `job` of every `STATE_SNAPSHOT`
(`attempt`, `maxAttempts`, `gate`, `sha`), the `vymalo.check` card of each source in each verification of each attempt (`check-<attempt>-<verification>-<source>`),
`vymalo.rework` (`rework-<attempt>`) and the subagent of the next attempt (`sub-<seq of the rework>`). Their event logs are
produced with the fake agent's `verify-*` scripts, whose commits are `<attempt as 40 hex digits>`. The reference client's
`expected/verify-green.json` shows the last state it holds: `done`, attempt 2, and one card per source and attempt.

The `verify-verifier-green.agui.json` and `verify-verifier-red.agui.json` goldens are the same story with a **verifier agent**
([`../agui.md`](../agui.md#the-verifier-as-a-subagent)): the verifier is a subagent of its own (`sub-verify-<verification>`,
named after the agent), started by the `pending` card of the `verifier` source and ended by its verdict
(`SUBAGENT_FINISHED` with `result: {"passed": …}`), so the green one has four subagents (the worker, the verifier, the worker,
the verifier). Their event logs are produced with the fake agent's `verify-reviewed` script (a worker that pushes and says what it
did) and the fake verifier's scripts `FindingsThenPass` and `AlwaysFail`.

The `catalog.agui.json`, `run-catalog.agui.json` and `connect-catalog.agui.json` goldens are the UI catalog a viewer and a requester read
([`../agui.md`](../agui.md#the-ui-catalog), ADR 0023): the `ui_catalog` event has **no frame**, so each run is an ordinary run, and the only trace
is `thread.uiCatalog` (`{catalogId, version, digest}`) in every `STATE_SNAPSHOT`: version 1 in the first job, version 2 from the second job on, and
still version 2 in the third, whose version-1 catalog was known already. The reference client's `expected/catalog.json` shows the last state it holds.

The `fork.agui.json` and `fork-blocked.agui.json` goldens are the forks a viewer reads ([`../agui.md`](../agui.md#forks), ADR 0029): the
parent's frames up to the cut, with the parent's ids and resume points (`run-1`, `evt-1` … `id: 5`), then **the marker as a run of
its own** (`run-<seq of thread_forked>`: `ACTIVITY_SNAPSHOT` `vymalo.fork` with the id `fork-<seq>`, a `STATE_SNAPSHOT` that says `done` and
`thread.forkedFrom`, `RUN_FINISHED` success), then the fork's own life: its first snapshot says `jobNumber: 2` and `forkedFrom`, and every
snapshot after the marker does. The copy's snapshots, before the marker, do not. The reference client's `expected/fork.json` shows the
last state it holds: three runs (the parent's, the marker's, the fork's), the marker among the messages as an activity.

The `steps.agui.json` and `steps-ask.agui.json` goldens are the nested steps a viewer reads
([`../agui.md`](../agui.md#nested-steps), ADR 0025): a sub-agent step is a **subagent** of the run
(`sub-step-<seq>`, started in the agent's invocation) and every step is a `vymalo.step` activity
(`step-<seq>`, said again with `replace: true` at each event of the step), a command is attributed to the sub-agent that
runs it, and a failed command is an activity and no more (the run goes on). In `steps-ask` the step is open when the agent asks:
its subagent suspends with the invocation (`suspended`, no interrupt ids of its own), and in the run that resumes the end of
the command and of the sub-agent only say their activities again, attributed to the invocation. The reference client's
`expected/steps.json` shows the tree it holds: both steps `completed` (or `failed`), with their paths and their `startedAt`.

The `working.agui.json` golden is working text and the turn's answer a viewer reads ([`../agui.md`](../agui.md#the-agents-words), ADR 0031):
the two assistant messages of the turn say what they are for on their `START`, `msg-3` `metadata["vymalo.purpose"]: "working"` (the sentence before
the command) and `msg-5` `"answer"` (the reply that ends the turn), the status that repeats the answer says no more, and a
message that is not marked has no member. The reference client reads both as plain assistant messages (`expected/working.json`).

The `stream.agui.json` golden is the live text a viewer reads ([`../agui.md`](../agui.md#live-text), ADR 0027): the reply `msg-3` is
opened by its first piece (`TEXT_MESSAGE_START` with `metadata["vymalo.live"]`), grows by two more (`CONTENT` with the `offset`, in UTF-16
code units, of what was said before), and is completed by the log's message: `CONTENT ""` with `{offset: 18, final: true}` and `END`
with `{final: true}` and the `id: 3`, while the status that repeats the words says no more. None of the live frames has an `id:`.
The reference client's `expected/stream.json` shows one message, `msg-3`, with the whole text.

The `ci.agui.json` golden is the CI gate a viewer reads (ADR 0017, [`../agui.md`](../agui.md#ci-results-vymalo-ci)): **one
run** across two attempts, the `vymalo.check` card of the source `ci` (`check-1-1-ci`, pending, then failed), between them
the `vymalo.ci` card of the report (`ci-<sha>-ci/build`, `replace: true`: conclusion, `passed`, `shortSha`, url and
summary), `vymalo.rework`, the next attempt's subagent, and the same again for a green report on the second commit.
Its event log is produced with the fake agent's `verify-ci` script and two reports the test sends through the inbox.

The `a2ui.agui.json` golden is the A2UI story a viewer reads: the surface as **two snapshots of one
activity** (`a2ui-3`, `replace: true`, the second carrying both payloads), the question, then the run of the
user's action (`vymalo.action`, no text message) and the answer. It goes through the reference client
like the others; the client's `expected/a2ui.json` shows the surface holding the operations of both.

### Run responses

[`agui/run-<name>.agui.json`](agui/) is what the **run route** (`POST /agui/agents/{agentId}`, see
[`../agui.md`](../agui.md#run-binding)) answered for each scripted behaviour, over real HTTP, in the same
frame format: the **requester's** projection, so the user messages the request itself carried are not
sent back. A scenario with two POSTs (`run-ask`: the question, then the answer as a `resume`) is the
responses in order, one run each. The consumer's thread id is `<thread-id>`; its message and run ids
(`msg-1`, `run-1`, `run-2`) are as it sent them.

| File | POSTs | Ends in |
|---|---|---|
| `run-echo.agui.json` | `echo hi` | success |
| `run-ask.agui.json` | `ask about branches`, then a `resume` answering `main` | interrupt, then success |
| `run-fail.agui.json` | `fail please` | `RUN_ERROR` `agent_failed` |
| `run-cancel.agui.json` | `slow work`, cancelled through `POST /api/threads/{id}/cancel` | cancelled |
| `run-release.agui.json` | `echo ship it` on `coder`, release `staging` in `forwardedProps` | success |
| `run-verify-green.agui.json` | `verify-red-once fix the login` with `forwardedProps["vymalo.gate"] = {"require": ["agent-checks"]}`: **one** response for two attempts | success, `job.attempt` 2 |
| `run-verify-red.agui.json` | `verify-red fix the login`, the same gate | `RUN_ERROR` `checks_failed` |
| `run-verify-verifier-green.agui.json` | `verify-reviewed fix the login`, `plain` requires the verifier in its own entry, so the run asks for nothing: **one** response for two attempts and two verifications | success, `job.attempt` 2 |
| `run-verify-verifier-red.agui.json` | the same, against a verifier that never passes | `RUN_ERROR` `checks_failed` |
| `run-ci.agui.json` | `verify-ci fix the login` with `forwardedProps["vymalo.gate"] = {"require": ["ci"]}`; the test reports CI through the inbox while the run is open: **one** response for two attempts | success, `job.attempt` 2 |
| `run-fork.agui.json` | `echo one` on a thread, a fork of it (`POST /api/threads/{id}/fork {after}`), then one POST on the **fork** with the messages the screen holds (`msg-1`, the copy's) and a new one (`msg-2`, `echo two`): accepted, one response, job 2, every snapshot with `forkedFrom` | success |

- **Producer:** `orchestrator/crates/e2e/tests/agui_run.rs` (`run_responses_match_docs_api_examples`);
  `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test agui_run` regenerates them; review the diff.
- **Consumers:** `tools/agui-conformance` reads them like the others.

### Connect streams

[`agui/connect-<name>.agui.json`](agui/) is what the **connect stream** (`GET /agui/threads/{threadId}/connect`,
see [`../agui.md`](../agui.md#connect-binding)) sent a **viewer**, over real HTTP, in the same frame format:
everything, including the user messages the requester holds already. The consumer's thread id is
`<thread-id>`.

| File | Thread | What the viewer reads |
|---|---|---|
| `connect-echo.agui.json` | `echo hi` | the replay of one finished run (`?mode=run`, so the stream ends) |
| `connect-ask.agui.json` | `ask about branches`, answered `main` | the replay of two runs on one stream: interrupt, then success |
| `connect-cancel.agui.json` | `slow work`, cancelled | the replay of a cancelled run |
| `connect-verify-green.agui.json` | `verify-red-once fix the login` under the gate | the replay of one run across two attempts: `verifying`, `vymalo.check` (failed), `vymalo.rework`, the second subagent, `vymalo.check` (passed), success |
| `connect-verify-red.agui.json` | `verify-red fix the login` under the gate | the same over three attempts, ending in `RUN_ERROR` `checks_failed` |
| `connect-verify-verifier-green.agui.json` | `verify-reviewed fix the login` under a gate that requires the verifier | the replay of one run: the verifier as a subagent (findings, then a pass), two attempts, success |
| `connect-verify-verifier-red.agui.json` | the same, against a verifier that never passes | three attempts and three verifier subagents, ending in `RUN_ERROR` `checks_failed` |
| `connect-ci.agui.json` | `verify-ci fix the login` under a CI gate, a red report then a green one | the replay of one run across two attempts with a `vymalo.ci` card for each report |
| `connect-cursor.agui.json` | `gate hold`, the client held log event 2 and reconnects with `Last-Event-ID: 2` | the **preamble** (`RUN_STARTED` of the same run, `SUBAGENT_STARTED`, `STATE_SNAPSHOT`, none with an `id:`), then the rest of the run |
| `connect-title.agui.json` | `echo hi`, finished, then renamed `Fix the build` | the replay of the run, every snapshot of it saying the title the thread has now, then the rename's own run: `RUN_STARTED`, `STATE_SNAPSHOT`, `RUN_FINISHED` with nothing between |
| `connect-fork.agui.json` | `echo one`, `echo two`, the second message edited into a fork (`{replace, text}`) | the replay of the fork: the first run of the parent (events 1-5), the marker run, then the edited message's run (job 2, `forkedFrom` in every snapshot) |
| `connect-fork-blocked.agui.json` | `ask about branches`, forked while it waits (`{after}`), then a run **on the fork** with the messages the screen holds (`msg-1`, the copy's) and `echo thanks` | the parent's run ending in its interrupt, the marker run, then the run of the new message: accepted, job 2; the question is not an interrupt of the fork |

[`agui/capabilities-<agent>.json`](agui/) is the `AgentCapabilities` document
(`GET /agui/agents/{agentId}/capabilities`, see [`../agui.md`](../agui.md#capabilities-document)) of the two
agents of the end-to-end world: `coder` (its card lists release channels) and `plain` (it does not). Not
`*.agui.json`: it is a document, not a stream, and the reference client has no reader for it; the
Rust tests validate it against the vendored schema (`#/$defs/AgentCapabilities`).

- **Producer:** `orchestrator/crates/e2e/tests/agui_connect.rs` (`connect_streams_match_docs_api_examples`,
  `capabilities_match_docs_api_examples`); `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test agui_connect`
  regenerates them; review the diff.
- **Consumers:** `tools/agui-conformance` reads the `connect-*` streams like the others, as one connect
  stream through `connectAgent` and each run alone through `runAgent`.
