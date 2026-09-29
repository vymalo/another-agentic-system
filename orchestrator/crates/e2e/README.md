# orch-e2e

End-to-end tests only: the AG-UI run route, the chat API, the dispatcher and the A2A adapter
against an in-process fake A2A agent over real HTTP, on the in-memory store
and on Postgres. The library is empty; everything is under `tests/`.

## Where it sits

The integration layer above all the other crates
([`orch-api`](../api/README.md), [`orch-app`](../app/README.md),
[`orch-agent-a2a`](../agent-a2a/README.md),
[`orch-store-postgres`](../store-postgres/README.md),
[`orch-testsupport`](../testsupport/README.md)); all of them are
dev-dependencies. It defines no port and no API (`publish = false`).

## API at a glance

None: there is no public API. `tests/common/mod.rs` is the shared harness. Each
scenario runs as two tests, `<name>::memory` and `<name>::postgres`.

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
| `e2e.rs` | the acceptance sequence: chat API, dispatcher, A2A adapter, agent |
| `agui_run.rs` | the AG-UI run route (`POST /agui/agents/{agentId}`) with the same stack: echo, ask and `resume`, fail, cancel (`RUN_FINISHED` cancelled), a retried POST attaches (also through another replica), refusals as problems, another owner's thread id is 404, release through `forwardedProps`, one log for both surfaces; every event validates against the vendored AG-UI schema; the responses are the goldens `docs/api/examples/agui/run-*.agui.json` |
| `agui_connect.rs` | the AG-UI connect stream and capabilities document: a client connected to a replica that is killed (with the agent's task parked) reconnects to another with `Last-Event-ID` and gets the preamble and exactly the missing suffix, no gap and no duplicate (the joined frames equal a fresh replay); viewers on several replicas each get the whole stream, the requester's response is the same run; from every resume point the rest of a two-run thread, on either replica; another owner's, a missing and a malformed thread id are one 404 problem before the stream, on every replica; the capabilities document is the live card in the spec's shape. Every event and document validates against the vendored schema; the streams are the goldens `docs/api/examples/agui/connect-*.agui.json` and `capabilities-*.json` |
| `deprecation.rs` | the composed router (AG-UI and the chat API): the four legacy operations answer with `Deprecation` (RFC 9745), the AG-UI operations, the resource API and health do not |
| `restart.rs` | the process dies mid-stream, a new one on the same database finishes with no gap and no duplicate |
| `replicas.rs` | several replicas on one database (Postgres only) |
| `sse_resume.rs` | SSE resume with `Last-Event-ID` |
| `blocked.rs` | `input-required` blocks the thread; a follow-up continues the same A2A task |
| `cancel.rs` | cancelling sends `CancelTask` |
| `release.rs` | release channels: discovery from the live card, selection on the wire |
| `agent_auth.rs` | bearer auth towards the agent |
| `agent_text.rs` | what the agent says reaches the chat as `agent_message` events, each once |
| `golden.rs` | golden transcripts, replayed by the web's `src/chat/golden.test.ts` |
| `wiremock_agent.rs` | the real stack against the compose WireMock agents (in-memory store), to keep the mocks honest; see [`dev/README.md`](../../../dev/README.md) |

```sh
cd orchestrator
ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test \
  cargo test -p orch-e2e
```

## See also

[`orch-testsupport`](../testsupport/README.md),
[`docs/mvp.md`](../../../docs/mvp.md).
