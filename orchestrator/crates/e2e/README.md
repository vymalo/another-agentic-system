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

None: there is no public API. `tests/common/mod.rs` is the shared harness. Each
scenario runs as two tests, `<name>::memory` and `<name>::postgres`. Threads are created and
continued over the AG-UI run route (`Chat::create_thread`, `Chat::follow_up`), like a consumer does;
the log as the core wrote it has no HTTP route, so `Chat::events` reads it in-process (a client from
`TestInstance::chat`).

## Features and environment

No Cargo features.

| Variable | Meaning |
|---|---|
| `ORCH_TEST_DATABASE_URL` | enables the `::postgres` variants and `replicas`; without it they print a notice and pass without running. Each test gets its own schema |
| `ORCH_TEST_MOCK_AGENT_URL`, `ORCH_TEST_MOCK_AGENT_RELEASES_URL` | base URLs of the WireMock stand-ins (`docker compose up -d --wait mock-agent mock-agent-releases`, ports 8081 and 8082); enable `wiremock_agent` |
| `UPDATE_GOLDEN` | `1` makes `golden` rewrite `docs/api/examples/*.events.json`, `agui_run` `docs/api/examples/agui/run-*.agui.json`, and `agui_connect` `connect-*.agui.json` and `capabilities-*.json`, instead of failing on a difference |

## Tests

| File | Covers |
|---|---|
| `e2e.rs` | the acceptance sequence: AG-UI run and connect, dispatcher, A2A adapter, agent (the connect stream's resume points are the log's `seq`); failures and chunked artifacts; two threads and two users side by side; the removed legacy interaction routes are 404 (405 for `POST /api/threads`) on the composed router |
| `agui_run.rs` | the AG-UI run route (`POST /agui/agents/{agentId}`) with the same stack: echo, ask and `resume`, fail, cancel (`RUN_FINISHED` cancelled), a retried POST attaches (also through another replica), refusals as problems, another owner's thread id is 404, release through `forwardedProps`, one log for both surfaces; every event validates against the vendored AG-UI schema; the responses are the goldens `docs/api/examples/agui/run-*.agui.json` |
| `agui_connect.rs` | the AG-UI connect stream and capabilities document: a client connected to a replica that is killed (with the agent's task parked) reconnects to another with `Last-Event-ID` and gets the preamble and exactly the missing suffix, no gap and no duplicate (the joined frames equal a fresh replay); viewers on several replicas each get the whole stream, the requester's response is the same run; from every resume point the rest of a two-run thread, on either replica; another owner's, a missing and a malformed thread id are one 404 problem before the stream, on every replica; the capabilities document is the live card in the spec's shape. Every event and document validates against the vendored schema; the streams are the goldens `docs/api/examples/agui/connect-*.agui.json` and `capabilities-*.json` |
| `agui_a2ui.rs` | A2UI through the real stack (fake agent listing the extension in its card): a surface reaches the requester and a viewer whole, the action returns through `forwardedProps.a2uiAction` and reaches the agent as an `application/a2ui+json` data part of the same task, unknown and oversized actions never reach the agent, a refused part is an error line and the run goes on, a surface in a message and in the question, a deleted surface, the capabilities document following the live card |
| `local_agent.rs` | local agents ([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)) through the real `App` and dispatcher, with `ByTransport<A2aAgentClient, LocalAgentClient>` and no HTTP (`orch-agent-adam`'s `testkit`): an echo thread completes with the answer in the completed status, a restart mid-task finishes with no gap and no duplicate (the second process steps the run again after both leases expire), and a cancel reaches the local task. Memory and Postgres; the orchestrator's tables and the local journal share one schema |
| `verify.rs` | the verification gate through the AG-UI run route and the fake agent's `verify-*` scripts ([ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md)): red once, one run across two attempts, the second delegation a new A2A task in the same context whose text has the findings and "attempt 2", `job` at attempt 2 in the snapshots and in the thread; red always fails after three attempts with `checks_failed` (`RUN_ERROR`); a run lowers the attempts and the target's gate applies; a follow-up that asks for another gate is a 409 and one that weakens or malforms it a 400, the loser of a race to create a thread is a 409, the gate of a job sent back as a request works (both spellings); a run that removes a required source, exceeds the cap, asks for `ci` or `verifier`, or is malformed is a 400 problem and creates nothing. Also the goldens `verify-green` and `verify-red` (`golden.rs`, `agui_run.rs`, `agui_connect.rs`) |
| `restart.rs` | the process dies mid-stream, a new one on the same database finishes with no gap and no duplicate |
| `replicas.rs` | several replicas on one database (Postgres only): a viewer's connect stream on a replica that does not dispatch sees the events another replica dispatched; a cancel through a replica that runs no dispatcher |
| `blocked.rs` | `input-required` and `auth-required` block the thread; a follow-up (a new AG-UI run on the thread) continues the same A2A task, also across an orchestrator restart |
| `cancel.rs` | cancelling sends `CancelTask` |
| `release.rs` | release channels: discovery from the live card, selection on the wire |
| `agent_auth.rs` | bearer auth towards the agent |
| `agent_text.rs` | what the agent says reaches the chat as `agent_message` events, each once |
| `golden.rs` | golden transcripts of the log (`docs/api/examples/*.events.json`), which `orch-agui-projection` projects into the AG-UI goldens the web's mock test `mock/golden.test.ts` reads; the first message enters through the application, not a surface, so the transcripts hold no consumer-chosen ids (`a2ui`'s action goes through the AG-UI run route) |
| `wiremock_agent.rs` | the real stack against the compose WireMock agents (in-memory store), to keep the mocks honest, including `red-once` and `red-always` under the gate; see [`dev/README.md`](../../../dev/README.md) |

```sh
cd orchestrator
ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test \
  cargo test -p orch-e2e
```

## See also

[`orch-testsupport`](../testsupport/README.md),
[`docs/mvp.md`](../../../docs/mvp.md).
