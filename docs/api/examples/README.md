# Golden event transcripts

What the real orchestrator emits, one file per scripted agent behaviour. They pin the *meaning*
of the events (kinds, `agent_status` spellings, failure shape, message finality), which
[`chat-api.yaml`](../chat-api.yaml) cannot: `EventData` is an untyped bag there.

| File | Agent script (first word of the message) | Ends in |
|---|---|---|
| `echo.events.json` | `echo`: working, artifact with the PR link, completed | `done` |
| `ask.events.json` | `ask`, then the answer `main`: blocked, resumed on the same task | `done` |
| `cancel.events.json` | `slow`, then Cancel | `cancelled` |
| `fail.events.json` | `fail`: `agent_status: failed` with `detail`, no `error` event | `failed` |
| `talk.events.json` | `talk`: status text, one final `agent_message`, the artifact | `done` |
| `release.events.json` | `echo` with release `staging`: the revision on every agent actor | `done` |

Ids and clocks are normalised: `threadId` is `<thread-id>`, `at` is `<timestamp>` and an agent
message's `messageId` is `<message-id>`.

- **Producer:** `orchestrator/crates/e2e/tests/golden.rs` (`transcripts_match_docs_api_examples`)
  runs each script through the real chat API, dispatcher and A2A adapter, and fails when a file
  differs. After an intended change, regenerate and review the diff:
  `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test golden`.
- **Consumers:** `web/src/features/chat/lib/golden.test.ts` maps every file with the chat surface's reducer
  and converters, and `web/mock/golden.test.ts` requires the mock server to tell the same story.

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
  the reference client (`@ag-ui/client` 1.0.0) in CI.

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
