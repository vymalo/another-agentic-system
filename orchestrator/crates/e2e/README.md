# orch-e2e

End-to-end tests only: the chat API, the dispatcher and the A2A adapter
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
| `UPDATE_GOLDEN` | `1` makes `golden` rewrite `docs/api/examples/*.events.json` instead of failing on a difference |

## Tests

| File | Covers |
|---|---|
| `e2e.rs` | the acceptance sequence: chat API, dispatcher, A2A adapter, agent |
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
