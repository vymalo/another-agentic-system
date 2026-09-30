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
[`orch-ports`](../ports/README.md), and on `a2a-server-lf` for the fake agent.
Its helpers panic on failure, by design.

## API at a glance

| Item | What |
|---|---|
| `FakeAgent`, `FakeAgentOptions`, `FakeReleases`, `Call`, `CallKind` | an in-process A2A 1.0 agent on `a2a-server-lf`, optional bearer auth and release-channels card, and A2UI (`FakeAgentOptions::ui_extensions` puts the extension URIs in the card, `set_ui_extensions` changes them while it runs; the scripts `ui`, `ui-msg`, `ui-status`, `ui-bad`, `ui-big` and `ui-delete` send surfaces, and `Call` records the renderer capabilities and the action messages a request carried); `spawn`, `card_url`, `endpoint(id, bearer)`, `calls`, `executions`, `cancels`, `release_gate`, `rpc_count`, `unauthorized_requests`, `stop` |
| `TestInstance` | the router (the resource API and the AG-UI surface) and a dispatcher on a real TCP port: `spawn`, `spawn_with` (API only with `None`), `kill` (a crash: no lease release, and the streams that are open break mid-body, so a client sees the connection drop), `shutdown`, `chat(user)`; `fast_dispatcher()` gives millisecond timings |
| `Chat` | a client acting as one user: the resource API (`get`, `post`, `thread`, `state`, `wait_state`, `cancel`, `as_user`, `anonymous`) and the AG-UI routes. Threads are made and continued the way a consumer does: `create_thread` / `try_create_thread` (a run of a first message under a fresh UUIDv7 thread id, the release in `forwardedProps`; the run's stream is dropped unread, the log does not depend on it) and `follow_up` (a new run on the thread; returns its stream). The raw AG-UI pieces: `agui_post` (the raw answer), `agui_run` (the stream of an accepted run), `agui_input` (builds a `RunAgentInput`), `agui_connect_raw` (the raw answer, `Last-Event-ID` and query verbatim), `agui_connect` (`last_event_id`, `mode_run`), `agui_capabilities`. A client from `TestInstance::chat(user)` can also read and write the log **in-process**, through the same `App` the routes use, because no HTTP route returns the log as the core wrote it: `events`, `wait_events` (the JSON of `orch_core::Event`), and `seed_thread` / `seed_message` (a thread or message with no surface in between, so no consumer-chosen ids: what a producer that is not AG-UI would write). `Chat::new(base_url, user)` has no such access (a real process under test) |
| `SseClient`, `Frame` | reads an AG-UI SSE stream: frames, `data:` plus an optional `id:` (`next_frame`, `frames_until` to a frame of a stream that goes on, `collect_frames` to the end of the response) |
| `eventually`, `eventually_within`, `DEFAULT_TIMEOUT`, `shape` | wait-until with a deadline instead of sleeping; `shape` lists the kind (and status or state) of each event |

The fake agent's behaviour is chosen by the first word of the user's message
(`echo` or anything else, `ask`, `gate`, `slow`, `chunks`, `fail`, `talk`,
`messages`, `auth`, and for the verification gate `verify-pass`, `verify-red-once` and `verify-red`, which report a
`branch` and a `checks` artifact and answer the gate's rework prompt as a new task of the same context); the table is
in `src/fake.rs`.

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
| `FAKE_CONTROL_ADDR` | `127.0.0.1:4020` | `POST /__control/<agent>/release-gate`, `GET /__control/<agent>/calls` |

## Features

None.

## Tests

The crate has no tests of its own; it is exercised by every crate that uses it
(`orch-agent-a2a`, `orch-e2e`, the `orchestrator` smoke test).

## See also

[`orch-e2e`](../e2e/README.md), [`orch-agent-a2a`](../agent-a2a/README.md).
