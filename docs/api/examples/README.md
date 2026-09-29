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
- **Consumers:** `web/src/chat/golden.test.ts` maps every file with the chat surface's reducer
  and converters, and `web/mock/golden.test.ts` requires the mock server to tell the same story.
