# orch-a2a-mapping

The pure mapping from A2A 1.0 values to the orchestrator's protocol-neutral
`AgentEnvelope`s and idempotency keys. No I/O, no async, no HTTP client: it
depends on the A2A value types (`a2a-lf`), `orch-core` and `orch-ports` only.

## Where it sits

A helper of the A2A adapter, split out so that the mapping (the part that
decides which idempotency key a status, artifact or message gets, and how
chunked artifacts are assembled) is a function of its inputs and can be
reused by any component that produces A2A values, not only the HTTP client.
Today [`orch-agent-a2a`](../agent-a2a/README.md) is its only user. Dependency
direction: `core` <- `ports` <- `a2a-mapping` <- `agent-a2a`
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md),
[ADR 0004](../../../docs/decisions/0004-closed-enums-over-dyn-registry.md)).

## API at a glance

| Item | What |
|---|---|
| `StreamMapper` (`Default`), `StreamMapper::map(StreamResponse) -> Vec<Result<AgentEnvelope, AgentError>>` | stream-local state (the task the stream belongs to, the artifact held back because more chunks may follow) and the mapping of one stream item |
| `snapshot(&Task) -> Result<TaskSnapshot, AgentError>` | a polled view of a task: every artifact (chunks merged), then the status, under the same keys as the live stream; an unspecified state is a protocol violation |
| A2UI parts | a `Data` part whose `mediaType` or `metadata.mimeType` is `application/a2ui+json` (in an agent message, a status message or an artifact) maps to one `AgentUpdate::Ui` envelope through `orch_core::check_operations`, or to `AgentUpdate::UiRejected` when it fails (wrong content, not an array, over the cap, unknown version or operation). Never text, never the artifact's JSON. The envelopes of a status message come before the status. Keys: `a2a:<task>:artifact:<id>:ui:<part>`, `a2a:msg:<id>:ui:<part>`, `a2a:<task>:status-msg:<id>:ui:<part>` (`Turn(<task>:status-ui:<state>:<part>)` without a message id) |
| `RELEASE_CHANNELS_URI` | the release-channels v1 extension URI (ADR 0008), under which an agent echoes the revision it ran; `orch-agent-a2a` re-exports it |

Idempotency keys (the dispatcher stores them so replays never duplicate chat
events):

| A2A value | key |
|---|---|
| artifact `X` of task `T` | `Task("a2a:T:artifact:X")` |
| agent message `M` | `Task("a2a:msg:M")` |
| status carrying message `M` | `Task("a2a:T:status-msg:M")` |
| status without a message | `Turn("T:status:<state>")` |

An artifact not marked `lastChunk: true` is held back until the next event
proves no continuation follows; see the module documentation in `src/lib.rs`
for chunking and the known limits (`Task.history` is not replayed).

```rust
use orch_a2a_mapping::{StreamMapper, snapshot};

let mut mapper = StreamMapper::default();
let envelopes = mapper.map(stream_item);          // per item of SendStreamingMessage
let polled = snapshot(&task)?;                    // per GetTask
```

## Features and environment

No Cargo features; reads no environment variables.

## Tests

Unit tests in `src/lib.rs`, offline: every state, keys, chunked and whole
artifacts, held-back artifacts, messages, snapshots and revisions; and A2UI parts in messages, status messages and artifacts (one envelope per part, the JSON never printed as an artifact, order before the status, keys equal on a poll and on the stream, both spellings of the media type, refusal of malformed, oversized, wrong-content and unsupported-version parts, a refusal not stopping the next part). They moved
here unchanged from `orch-agent-a2a`; the adapter's behaviour through the
mapping stays covered by its `tests/against_fake_agent.rs` and
`tests/conformance.rs`.

## See also

[`orch-agent-a2a`](../agent-a2a/README.md),
[`orch-ports`](../ports/README.md), [`orch-core`](../core/README.md).
