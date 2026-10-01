# orch-e2e

End-to-end tests only: the AG-UI routes, the resource API, the dispatcher and the A2A adapter
against an in-process fake A2A agent over real HTTP, on the in-memory store
and on Postgres. The library is empty; everything is under `tests/`.

## Where it sits

The integration layer above all the other crates
([`orch-api`](../api/README.md), [`orch-app`](../app/README.md),
[`orch-agent-a2a`](../agent-a2a/README.md),
[`orch-agent-adam`](../agent-adam/README.md),
[`orch-store-postgres`](../store-postgres/README.md),
[`orch-testsupport`](../testsupport/README.md)); all of them are
dev-dependencies. It defines no port and no API (`publish = false`).

## API at a glance

None: there is no public API. `tests/common/mod.rs` is the shared harness (`World`, whose `Setup.gate` is the verification gate its instances start threads under and `Setup.reviewer` the script of an optional third agent, `reviewer`, that plays the verifier (`verified_by_reviewer(script)` is the world of the verifier goldens), and `Node`, an application instance without HTTP or a dispatcher that can run an `InboxWorker` and play a surface or the agent). Each
scenario runs as two tests, `<name>::memory` and `<name>::postgres`. Threads are created and
continued over the AG-UI run route (`Chat::create_thread`, `Chat::follow_up`), like a consumer does;
the log as the core wrote it has no HTTP route, so `Chat::events` reads it in-process (a client from
`TestInstance::chat`).

## Features and environment

No Cargo features.

| Variable | Meaning |
|---|---|
| `ORCH_TEST_DATABASE_URL` | enables the `::postgres` variants and `replicas`; without it they print a notice and pass without running. Each test gets its own schema |
| `ORCH_TEST_MOCK_AGENT_URL`, `ORCH_TEST_MOCK_AGENT_RELEASES_URL`, `ORCH_TEST_MOCK_VERIFIER_URL` | base URLs of the WireMock stand-ins (`docker compose up -d --wait mock-agent mock-agent-releases mock-verifier`, ports 8081, 8082 and 8083); enable `wiremock_agent` (the verifier tests need the first and the last) |
| `UPDATE_GOLDEN` | `1` makes `golden` rewrite `docs/api/examples/*.events.json`, `agui_run` `docs/api/examples/agui/run-*.agui.json`, and `agui_connect` `connect-*.agui.json` and `capabilities-*.json`, instead of failing on a difference |

## Tests

| File | Covers |
|---|---|
| `e2e.rs` | the acceptance sequence: AG-UI run and connect, dispatcher, A2A adapter, agent (the connect stream's resume points are the log's `seq`); failures and chunked artifacts; two threads and two users side by side; the removed legacy interaction routes are 404 (405 for `POST /api/threads`) on the composed router |
| `mcp.rs` | the MCP server ([ADR 0019](../../../docs/decisions/0019-mcp-server-over-streamable-http.md)) with an rmcp client over streamable HTTP, the real dispatcher and the A2A adapter to a fake agent: a job started with `start_job` ends `done` and is in the same owner's thread list of the resource API (its first message has `origin: mcp`); `answer` unblocks a job and a message to a finished one is refused; `cancel_job` reaches the agent; a retried `start_job` on another replica is the same job; `wait_for_job` on a replica that delivers nothing follows a job another replica delivers (one progress notification per event, then the finished job); **a replica killed mid-wait**, then the call repeated on the other with the cursor of the last notification: together the two calls report every event once |
| `agui_run.rs` | the AG-UI run route (`POST /agui/agents/{agentId}`) with the same stack: echo, ask and `resume`, fail, cancel (`RUN_FINISHED` cancelled), a retried POST attaches (also through another replica), refusals as problems, another owner's thread id is 404, release through `forwardedProps`, one log for both surfaces; every event validates against the vendored AG-UI schema; the responses are the goldens `docs/api/examples/agui/run-*.agui.json` |
| `agui_connect.rs` | the AG-UI connect stream and capabilities document: a client connected to a replica that is killed (with the agent's task parked) reconnects to another with `Last-Event-ID` and gets the preamble and exactly the missing suffix, no gap and no duplicate (the joined frames equal a fresh replay); viewers on several replicas each get the whole stream, the requester's response is the same run; from every resume point the rest of a two-run thread, on either replica; another owner's, a missing and a malformed thread id are one 404 problem before the stream, on every replica; the capabilities document is the live card in the spec's shape. Every event and document validates against the vendored schema; the streams are the goldens `docs/api/examples/agui/connect-*.agui.json` and `capabilities-*.json` |
| `agui_a2ui.rs` | A2UI through the real stack (fake agent listing the extension in its card): a surface reaches the requester and a viewer whole, the action returns through `forwardedProps.a2uiAction` and reaches the agent as an `application/a2ui+json` data part of the same task, unknown and oversized actions never reach the agent, a refused part is an error line and the run goes on, a surface in a message and in the question, a deleted surface, the capabilities document following the live card |
| `local_agent.rs` | local agents ([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)) through the real `App` and dispatcher, with `ByTransport<A2aAgentClient, LocalAgentClient>` and no HTTP (`orch-agent-adam`'s `testkit`): an echo thread completes with the answer in the completed status, a restart mid-task finishes with no gap and no duplicate (the second process steps the run again after both leases expire), and a cancel reaches the local task. Memory and Postgres; the orchestrator's tables and the local journal share one schema |
| `ui_catalog.rs` | the UI catalog ([ADR 0023](../../../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md)) end to end on both stores, through the AG-UI run route: the first run records its catalog first (and every snapshot names it), the same request again attaches and writes nothing twice, a newer version on the next job is recorded and an older one is not current, a viewer reads the history, and a catalog that breaks a rule is a 400 or a 413 with no thread and nothing sent to the agent; and the extension (`ui-catalog/v1`): an agent that lists it with A2UI gets version 1 inline in the first message, version 2 inline when a newer screen opens the chat, and only `{version: 2, inline: false}` when an older screen joins or a message carries none; a card without the URI is sent no catalog though the log holds it; the capabilities document lists the extensions the card lists |
| `followup.rs` | a thread is a conversation ([ADR 0020](../../../docs/decisions/0020-a-thread-is-a-conversation.md), [ADR 0021](../../../docs/decisions/0021-context-across-a2a-tasks.md)), on both stores, through the real dispatcher and the A2A adapter against the fake agent: a message after `done` or `cancelled` is a new task of the same context that names the previous task (`referenceTaskIds`), and the log has one `job_started` |
| `verify.rs` | the verification gate through the AG-UI run route and the fake agent's `verify-*` scripts ([ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md)): red once, one run across two attempts, the second delegation a new A2A task in the same context whose text has the person's request (fenced, before the findings), the findings and "attempt 2", and the thread's export (`GET /api/threads/{id}/export`) holds the whole job and the log as listed, `job` at attempt 2 in the snapshots and in the thread; red always fails after three attempts with `checks_failed` (`RUN_ERROR`); a run lowers the attempts and the target's gate applies; a follow-up that asks for another gate is a 409 and one that weakens or malforms it a 400, the loser of a race to create a thread is a 409, the gate of a job sent back as a request works (both spellings); a run that removes a required source, exceeds the cap, asks for `verifier` or the `ci` settings (per thread), or is malformed is a 400 problem and creates nothing. Also the goldens `verify-green` and `verify-red` (`golden.rs`, `agui_run.rs`, `agui_connect.rs`) |
| `verifier.rs` | the verifier agent in the gate ([ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md), slice 10), against a fake verifier (`VerifierScript`): findings, a rework and a pass at attempt 2 (one run, the verifier a subagent of its own named after it, its context `<thread>-verify-<attempt>-<verification>` and never the worker's, the prompt naming the commit, the attempt and the quoted task and summary, the findings quoted to the worker, none of the verifier's words becoming the worker's); out of attempts gives `checks_failed`; no verdict and a garbled one are failed checks; a flood of findings is capped; a verifier that hangs holds the thread (`blocked`, no attempt spent, the verifier told to stop, the row dropped: the deadline is the core's own timer fired by the inbox worker); a verifier whose task fails, or that cannot be reached, holds it too; a message during a verification abandons it and the next one has a context of its own; a crash in the middle of a verification gives one verdict and the verifier is asked once. Memory and Postgres |
| `inbox.rs` | the inbox, with the webhook and the agent played through `App` (`Node`; the real route is `webhook.rs`): a deadline scheduled by a gated completion (the fake agent's `verify-ci` script) fires and blocks the thread, spending no attempt; a row claimed by a process that died is applied once by the next when its lease lapses, and the late process is fenced; a report received before its watch is parked and applied after the commit that starts the watch, and its redelivery is a duplicate. Waits are `eventually` on the store, never a sleep |
| `webhook.rs` | the generic CI webhook (`orch-surface-webhook`) through the real HTTP route, the inbox worker (driven with `tick`) and the pure core, on both stores, with a `FixedClock` the test holds (which is also the clock the route reads): a report that beats its watch is parked, matched by the agent's push and the job is done; a red report reworks with the report in the findings, a report about the old commit changes nothing, the green one for the new commit finishes; with no report the deadline blocks the thread (`ci_timeout`, no attempt spent, one second early is not enough); GitHub's own deliveries through the same path, on a gate that names the check `build` (a fork's green run, an unnamed workflow, a `check_suite` and a `skipped` lint arrive first and decide nothing, a failed `build` `check_run` reworks with its summary as a finding, a `push` changes nothing, a `CI` workflow is a card that does not count, a green `build` for the new commit ends the job); a refused delivery changes nothing |
| `restart.rs` | the process dies mid-stream, a new one on the same database finishes with no gap and no duplicate |
| `replicas.rs` | several replicas on one database (Postgres only): a viewer's connect stream on a replica that does not dispatch sees the events another replica dispatched; a cancel through a replica that runs no dispatcher |
| `blocked.rs` | `input-required` and `auth-required` block the thread; a follow-up (a new AG-UI run on the thread) continues the same A2A task, also across an orchestrator restart |
| `cancel.rs` | cancelling sends `CancelTask` |
| `release.rs` | release channels: discovery from the live card, selection on the wire |
| `agent_auth.rs` | bearer auth towards the agent |
| `agent_text.rs` | what the agent says reaches the chat as `agent_message` events, each once |
| `golden.rs` | golden transcripts of the log (`docs/api/examples/*.events.json`, `ci` among them: the fake agent's `verify-ci`, a red and a green `ci/build` reported through the inbox, `drive_ci` in `common/mod.rs`), which `orch-agui-projection` projects into the AG-UI goldens the web's mock test `mock/golden.test.ts` reads; the first message enters through the application, not a surface, so the transcripts hold no consumer-chosen ids (`a2ui`'s action goes through the AG-UI run route, and so do the three runs of `catalog`, the UI catalog of ADR 0023 in versions 1, 2 and 1 again, whose log therefore holds the consumer's message and run ids); the verification gate's four (`verify-green`, `verify-red` and, with a verifier agent, `verify-verifier-green`, `verify-verifier-red`) run under the gate their world gives `plain` (`world_for`) |
| `wiremock_agent.rs` | the real stack against the compose WireMock agents (in-memory store), to keep the mocks honest, including `red-once` and `red-always` under the gate and the mock verifier's findings and pass (`push-flawed`, `push-clean`: what the orchestrator really sends a verifier matches the WireMock mappings); see [`dev/README.md`](../../../dev/README.md) |

```sh
cd orchestrator
ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test \
  cargo test -p orch-e2e
```

## See also

[`orch-testsupport`](../testsupport/README.md),
[`docs/mvp.md`](../../../docs/mvp.md).
