# orch-embed

The orchestrator **embedded in an application** as a library: the same core, the same ports and the
same dispatcher as the server, with a thread's log in SQLite on the device and **no HTTP server**
(ADR 0076 decision 4; epic [#382](https://github.com/vymalo/another-agentic-system/issues/382)
slice E3).

## Where it sits

A **composition root**, not a service and not an adapter. It wires `orch_app::App` and
[`orch_app::Dispatcher`](../app/README.md) to [`orch-store-sqlite`](../store-sqlite/README.md) and
the adapters a local application has, and offers the chat API's operations and the AG-UI projection
as Rust calls. It reimplements nothing the application already owns: a thread is created and a
message submitted through `App`, the dispatcher is the application's, and the AG-UI events are
[`orch_agui_projection::Projector`](../agui-projection/README.md)'s fold of the log.

| Port | Adapter this root chooses |
|---|---|
| `ThreadStore` (and its supertraits `UsageLedger`, `ThreadSearch`, `PromptStore`, `UserToolServerStore`, `PendingAuthorizations`) | `SqliteStore` |
| `Wakeup` | `SqliteWakeup`, in process |
| `AgentClient` | the caller's, generic |
| `AgentRegistry` | the static `AgentDirectory` the caller gives |
| `Clock`, `IdGen` | the system clock and UUIDv7 ids |
| `ChatModel` | the application's, through `EmbedPorts` (`Embedded::open_with`); none by default: a title is the first words of its message, and no description task runs |
| `Authenticator` | `RefuseAll`: the application states the person, and no bearer is checked in process |
| `ArtifactStore` | the application's file store, through `EmbedPorts`; none by default |
| `UserToolPorts` | the application's bundle, through `EmbedPorts`: its `SecretStore` sits behind `orch-credential-broker`, which the application composes (this crate stays free of `axum`); off by default |

`bin/orchestrator` stays the server's composition root and does not depend on this crate. The
dependency direction is one way: this crate knows `orch-app`, `orch-ports`, `orch-core`,
`orch-store-sqlite` and the two pure AG-UI crates, and none of them knows it.

## API at a glance

```rust
use orch_app::{AgentDirectory, AgentEntry};
use orch_core::{AgentId, UserId};
use orch_embed::Embedded;
use orch_ports::AgentEndpoint;

let embed = Embedded::open("orch.sqlite", UserId::new("me@example.com"), directory, agents).await?;
let thread = embed.create_thread(&AgentId::new("helper"), "hello").await?;
let events = embed.collect_agui(thread.id).await?;   // until RUN_FINISHED / RUN_ERROR
```

| Item | What |
|---|---|
| `Embedded::open(path, person, directory, agent_client)` | opens and migrates the SQLite file, builds `App` over the ports above, and starts the dispatcher on the current Tokio runtime |
| `Embedded::create_thread(agent, text)` | creates a thread whose first message is queued, returns its `ThreadRecord` |
| `Embedded::post_message(thread, text)` | appends a message and queues its delegation |
| `Embedded::read_thread(thread)` | the thread as the log holds it |
| `Embedded::events(thread)` | every event of the thread, oldest first |
| `Embedded::stream_agui(thread)` | a `Stream` of the thread's AG-UI events, from the beginning to the run's `RUN_FINISHED` or `RUN_ERROR` |
| `Embedded::collect_agui(thread)` | the same stream, collected |
| `Embedded::app()` | the `Arc<App<Stack<A>>>`, for an operation this root does not wrap |
| `Embedded::shutdown()` | cancels the dispatcher and waits for it |

Dropping an `Embedded` cancels the dispatcher. A second process that opens the same file is refused
by the store's lock (`StoreError::Refused("already_open")`): a thread has one home, and one process
writes it (ADR 0076 decision 6).

## The example

`examples/run_thread.rs` runs one thread on the in-process scripted agent from `orch-ports`'s
`testkit` feature (the fake agent lives with the example, as a test dependency) and asserts the
AG-UI events it streams: one `RUN_STARTED`, one `TEXT_MESSAGE_START` from the assistant, the whole
reply as `TEXT_MESSAGE_CONTENT` deltas, one `TEXT_MESSAGE_END`, one successful `RUN_FINISHED`, and
no `RUN_ERROR`.

```sh
cd orchestrator
cargo run -p orch-embed --example run_thread --locked
```

`examples/kill_resume.rs` is the durability proof (ADR 0076, sys#382 E4b). It runs itself in three
processes: the first creates a thread with the scripted local agent (`testkit` of `orch-agent-adam`,
the `steps 5` script: five journaled steps, each an artifact) on two SQLite files and is killed with
SIGKILL after the second; the second opens the same files and waits for the thread to be `Done`,
then asserts the log holds `step-1` to `step-5` once each, in order. It takes about 30 s: the
dispatcher's job lease is 30 s. `KILL_RESUME_DISABLED=1` starts the second process without a
worker, so nothing resumes and the example fails.

```sh
cargo run -p orch-embed --example kill_resume --locked
```

CI runs both examples in the `embed` job of `.github/workflows/orchestrator.yml`, together with
`cargo fmt --check`, `cargo clippy -p orch-embed --all-targets -- -D warnings` and
`cargo test -p orch-embed`. The same job fails when the embedded build's normal dependency tree
holds `axum`, `sqlx`, `tokio-postgres` or `hyper`: a request from ADR 0076 decision 4, checked on
the tree rather than on a planted crate.

## The chat contract

`stream_agui_from` (replay after a seq), `post_message_with_id` (a client message id: a retry is one
message), `EmbedPorts::admission` (an `Admission` that answers `Limit`, the 402, one `CUSTOM` event
`vymalo.limit_reached`) and `EmbedPorts::min_client_version` with `with_client_version` (the 426
gate). The contract is in [`docs/api/agui.md`](../../../docs/api/agui.md) "Chat contract of an
application client"; `tests/chat_contract.rs` proves each piece. `ClientVersion` lives in `orch-core`
and `orch-api` re-exports it.

## What it does not build yet

ADR 0076's later slices are not this crate's:

- **E4**: the local agent adapter (`orch-agent-adam` driving `adam-engine` on adam's SQLite store).
  The example supplies an `AgentClient` of its own, which is what an application embedding its own
  local agents will do once E4 lands.
- The application's own filesystem `ArtifactStore` and keychain `SecretStore` adapters (the ports
  to hand them through, `EmbedPorts`, are built; `tests/ports.rs` proves the supplied model is the
  one asked, and the in-memory file store is the fake that passes its testkit).
- **E5**: the compile check of the embedded set for iOS, Android, macOS and Linux, and its size.

A thread on the device is read and written through this root only; it is not shared with a
deployment (ADR 0076 decision 6).
