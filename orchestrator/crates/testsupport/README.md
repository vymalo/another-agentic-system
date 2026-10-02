# orch-testsupport

Test-only helpers shared by the adapter and end-to-end tests: an in-process
fake A2A agent, a test instance of the orchestrator over real HTTP, and small
HTTP/SSE clients. Nothing here ships (`publish = false`, not in the image).

## Where it sits

A dev-dependency of [`orch-agent-a2a`](../agent-a2a/README.md),
[`orch-e2e`](../e2e/README.md) and the
[`orchestrator`](../../bin/orchestrator/README.md) binary's tests. It depends on
[`orch-app`](../app/README.md), [`orch-api`](../api/README.md),
[`orch-surface-agui`](../surface-agui/README.md) (mounted by `TestInstance`, like the default `ORCH_SURFACES=agui`) and
[`orch-ports`](../ports/README.md), and on `a2a-server-lf` for the fake agent and on `rmcp` (its client) for the
`thread-tools` script.
Its helpers panic on failure, by design.

## API at a glance

| Item | What |
|---|---|
| `FakeAgent`, `FakeAgentOptions`, `VerifierScript`, `FakeReleases`, `Call` (with the message's `reference_task_ids`, its `ui-catalog/v1` metadata `ui_catalog` and the catalogs it carried inline, `inline_catalogs`, and its `thread-tools/v1` metadata `thread_tools`, the `{url, token, expiresAt}` grant of the thread's endpoint), `CallKind` | an in-process A2A 1.0 agent on `a2a-server-lf`, optional bearer auth and release-channels card, and A2UI (`FakeAgentOptions::extensions` lists extensions of the orchestrator's own by URI and `accepts_inline_catalogs` makes its A2UI entries say `acceptsInlineCatalogs: true` (`FakeAgent::set_extensions` and `set_accepts_inline_catalogs` change them while it runs), `FakeAgentOptions::ui_extensions` puts the extension URIs in the card, `set_ui_extensions` changes them while it runs; the scripts `ui`, `ui-msg`, `ui-status`, `ui-bad`, `ui-big` and `ui-delete` send surfaces, and `choices` asks with a `Choices` of the web's own catalog and echoes the answers it is sent (`answered: ui-action answer db=pg auth=none deploy=k8s,compose`), and `Call` records the renderer capabilities and the action messages a request carried); `spawn`, `card_url`, `endpoint(id, bearer)`, `calls`, `executions`, `cancels`, `release_gate`, `rpc_count`, `unauthorized_requests`, `stop` |
| `TestInstance` | the router (the resource API and the AG-UI surface) and a dispatcher on a real TCP port: `spawn`, `spawn_with` (API only with `None`), `spawn_with_surfaces` (more surfaces beside AG-UI, built by the caller over the same `App`, for example the MCP server or the thread tools), `spawn_on` (the same on a listener the caller bound beforehand, for a test that has to tell something its own address before the instance exists: the base URL of the thread tools), `kill` (a crash: no lease release, and the streams that are open break mid-body, so a client sees the connection drop), `shutdown`, `chat(user)`; `fast_dispatcher()` gives millisecond timings |
| `Chat` | a client acting as one user: the resource API (`get`, `post`, `thread`, `state`, `wait_state`, `cancel`, `rename` (`PATCH /api/threads/{id}` with a `title`, as `(status, body)`), `as_user`, `anonymous`) and the AG-UI routes. Threads are made and continued the way a consumer does: `create_thread` / `try_create_thread` (a run of a first message under a fresh UUIDv7 thread id, the release in `forwardedProps`; the run's stream is dropped unread, the log does not depend on it) and `follow_up` (a new run on the thread; returns its stream). The raw AG-UI pieces: `agui_post` (the raw answer), `agui_run` (the stream of an accepted run), `agui_input` (builds a `RunAgentInput`), `agui_connect_raw` (the raw answer, `Last-Event-ID` and query verbatim), `agui_connect` (`last_event_id`, `mode_run`), `agui_capabilities`. A client from `TestInstance::chat(user)` can also read and write the log **in-process**, through the same `App` the routes use, because no HTTP route returns the log as the core wrote it: `events`, `wait_events` (the JSON of `orch_core::Event`), and `seed_thread` / `seed_message` (a thread or message with no surface in between, so no consumer-chosen ids: what a producer that is not AG-UI would write). `Chat::new(base_url, user)` has no such access (a real process under test) |
| `SseClient`, `Frame` | reads an AG-UI SSE stream: frames, `data:` plus an optional `id:` (`next_frame`, `frames_until` to a frame of a stream that goes on, `collect_frames` to the end of the response) |
| `ui_catalog(version)`, `with_ui_catalog(version)`, `UI_CATALOG_ID`, `integral_numbers(&Value)` | the UI catalog as the web sends it ([ADR 0023](../../../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md)): `ui_catalog` is `{catalogId, version, digest, catalog}` with the real digest (version 1 has `Text`, later ones add `Column`; each version has a digest of its own), `with_ui_catalog` the `extra` of `Chat::agui_input` that makes a run carry it, `integral_numbers` writes whole numbers as integers (an A2A server holds metadata numbers as doubles, so an inline catalog reads `256.0`; the digest of what an agent received is the sent one only after this) |
| `Call::activates_text_stream()`, `STREAM_PIECES`, `stream_text()`, `stream_id(task)` | streamed text ([`text-stream/v1`](../../../docs/api/text-stream-v1.md)): whether a call asked for the extension, and the reply the fake agent's `stream` scripts send (seven chunks about 150 ms apart, the stream id `<task>-reply`) |
| `Call::message_extensions`, `Call::activates_steps()`, `FakeAgent::subscription_extensions()` | what the orchestrator activated: the message's own `extensions`, whether `steps/v1` is in the header **and** the message, and the `A2A-Extensions` header of each `SubscribeToTask` (a resubscribe). The scripts `steps-io-bad` (a tool step whose `input` is a string and whose `output` has no text: both are dropped, the step is kept) and `steps-io-big` (a 5000-character argument and a 20 000-character result: both are cut), and `steps` (a sub-agent step `OpenCode`, a command `npm test` under it that is called with `{command, cwd, env}` (its `NPM_TOKEN` is the core's to redact, [ADR 0030](../../../docs/decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)) and fails with `1 failed` and the error text as its output, the sub-agent's end, the agent's words and `completed`; the transcript of the `steps` golden), `steps-ask` (the sub-agent and a command that is `waiting` when the agent asks `Allow rm -rf build?`; the answer ends both and completes: the `steps-ask` golden) and `steps-chatty` (one step reported running twenty-one times, then ended: what the log's bound is tested with) report steps the way an agent that speaks [`steps/v1`](../../../docs/api/steps-v1.md) does, whether or not the request activated the extension (list it with `FakeAgentOptions::extensions` or `FAKE_AGENT_EXTENSIONS=steps`) |
| `call_back(Option<Value>) -> String` | the agent's side of the thread tools ([`thread-tools/v1`](../../../docs/api/thread-tools-v1.md)): reads `{url, token}` of a grant, calls the endpoint with rmcp's own client, lists the tools, calls `get_ui_catalog` twice (the second time with the digest it was given) and returns one line (`thread-tools: tools=get_ui_catalog; catalog=<id> v2 <digest>; again unchanged=true`, `…; no catalog: …`, `no grant`, `refused: …`); what the fake agent's `thread-tools` script reports |
| `eventually`, `eventually_within`, `DEFAULT_TIMEOUT`, `shape` | wait-until with a deadline instead of sleeping; `shape` lists the kind (and status or state) of each event |

The fake agent's behaviour is chosen by the first word of the user's message
(`echo` or anything else, `ask`, `gate`, `slow`, `chunks`, `fail`, `talk`,
`messages`, `auth`, `recall` (answers `recalled: <the first line of the conversation the message was told>`, or `recalled: nothing`: what a fork's first task carries, [ADR 0029](../../../docs/decisions/0029-forking-a-thread-copies-its-log.md); every script reads the message **after** that conversation, and `Call::text` holds all of it), `thread-tools` (calls the thread's MCP endpoint back with the grant of its message, [`call_back`]), and for the verification gate `verify-pass`, `verify-red-once` and `verify-red`, which report a
`branch` and a `checks` artifact and answer the gate's rework prompt as a new task of the same context, and `verify-ci`,
which reports only the `branch` artifact, like an agent that pushed and leaves the checking to CI, and `verify-reviewed`, which also says what it did (`VERIFY_SUMMARY`) before it pushes, like an agent that leaves the checking to a verifier); the table is
in `src/fake.rs`. `VERIFY_REPOSITORY` and `verify_commit(attempt)` name what the `branch` artifact holds.

An agent can play the **verifier** of the gate (ADR 0018, slice 10): `FakeAgentOptions::verifier: Some(VerifierScript)` makes every message it gets a request to review a commit, answered with a `verdict` artifact as the script says: `FindingsThenPass` (rejects the first attempt's commit, `verify_commit(1)`, with `VERIFIER_FINDING`, and passes any other), `AlwaysFail`, `AlwaysPass`, `NoVerdict`, `Garbled` (a `passed` that is not a boolean), `Flood` (sixty long findings), `Hang` (working until cancelled), `GatedPass` (passes after `release_gate`) and `Broken` (a failed task). `Call::text` is the prompt and `Call::context_id` the verifier's context.

```rust
use orch_testsupport::{FakeAgent, FakeAgentOptions};

let agent = FakeAgent::spawn(FakeAgentOptions::default()).await;
let endpoint = agent.endpoint("coder", None);   // an orch_ports::AgentEndpoint
```

### `orch-fake-agent` (executable)

`src/bin/orch-fake-agent.rs` serves two scripted agents plus a control server
for the browser system tests (`web/e2e-system`, see
[`web/README.md`](../../../web/README.md)). It is never part of the image
(the image builds `--package orchestrator` only).

| Variable | Default | |
|---|---|---|
| `FAKE_CODER_ADDR` | `127.0.0.1:4021` | the `coder` agent, with the sample release channels |
| `FAKE_PLAIN_ADDR` | `127.0.0.1:4022` | the `plain` agent, no extension |
| `FAKE_AGENT_EXTENSIONS` | none | comma-separated extensions both agents list in their cards: `text-stream`, `ui-catalog` (also lists A2UI v0.9.1 with `acceptsInlineCatalogs: true`, so the catalog arrives inline), `thread-tools`, `steps`, `mentions`; `GET /__control/<agent>/calls` then shows `uiCatalog`, `threadTools` (the grant of a message) and `inlineCatalogs` of each message |
| `FAKE_CONTROL_ADDR` | `127.0.0.1:4020` | `POST /__control/<agent>/release-gate`, `GET /__control/<agent>/calls` |

## Features

None.

## Tests

The crate has no tests of its own; it is exercised by every crate that uses it
(`orch-agent-a2a`, `orch-e2e`, the `orchestrator` smoke test).

## See also

[`orch-e2e`](../e2e/README.md), [`orch-agent-a2a`](../agent-a2a/README.md).
