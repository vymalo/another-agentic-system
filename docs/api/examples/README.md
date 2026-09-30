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
| `a2ui.events.json` | `ui`, on an agent whose card lists the A2UI extension: a surface in two artifacts (`ui_surface` twice), the question, then the user's action through the AG-UI run route (`ui_action`) and the answer | `done` |

Ids and clocks are normalised: `threadId` is `<thread-id>`, `at` is `<timestamp>` and an agent
message's `messageId` is `<message-id>`.

- **Producer:** `orchestrator/crates/e2e/tests/golden.rs` (`transcripts_match_docs_api_examples`)
  runs each script through the real application (the first message enters through `App`, so the log holds no
  consumer-chosen ids; the action of `a2ui`: the AG-UI run route), dispatcher and A2A adapter, and fails when a file
  differs. `orch-api`'s `tests/contract.rs` validates every event of every file against the contract's `Event` schema. After an intended change, regenerate and review the diff:
  `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test golden`.
- **Consumers:** `web/mock/golden.test.ts` drives every scenario through the mock server's AG-UI routes
  and requires the connect stream to be the golden `agui/<name>.agui.json` below, so the mock tells the
  same story, `a2ui` included (a surface, the question, and the action that answers it through
  `forwardedProps.a2uiAction`). The web renders the AG-UI goldens, not these event logs: see the next section.

## AG-UI streams

[`agui/<name>.agui.json`](agui/) is what a **viewer** (an AG-UI connect stream, see
[`../agui.md`](../agui.md)) is shown for the same log: the frames `orch-agui-projection` produces
from each `<name>.events.json`, as an array of `{"id"?: <seq>, "event": <AG-UI event>}`. `id` is the
SSE `id:` a client resumes from, and is present only on the last frame of a log event with no text
message open. `threadId` is `<thread-id>` (a real thread id in any stream); the runs are
`run-<seq>`, the invocations `sub-<seq>`, and the interrupt `int-<seq>`.

- **Producer:** `orchestrator/crates/agui-projection/tests/golden.rs` projects the `*.events.json`
  files above (with placeholders made real) and fails when a file differs.
  `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test golden` regenerates them; review the
  diff.
- **Consumers:** the same test checks every event against the vendored AG-UI schema and every
  stream against the well-formedness rules; `tools/agui-conformance` feeds the frames, as SSE, through
  the reference client (`@ag-ui/client` 1.0.0) in CI; `web/mock/golden.test.ts` requires the mock
  server to produce them, and `web/src/features/chat/lib/agui/runtime-goldens.dom.test.tsx` runs them
  (and the `connect-*` ones) through the runtime the chat surface uses, `@assistant-ui/react-ag-ui`
  with `web/patches` applied, and checks the transcript.

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
| `connect-cursor.agui.json` | `gate hold`, the client held log event 2 and reconnects with `Last-Event-ID: 2` | the **preamble** (`RUN_STARTED` of the same run, `SUBAGENT_STARTED`, `STATE_SNAPSHOT`, none with an `id:`), then the rest of the run |

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
