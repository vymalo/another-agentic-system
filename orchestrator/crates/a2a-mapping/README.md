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
| Steps (`steps/v1`, [ADR 0025](../../../docs/decisions/0025-nested-steps-events-carry-their-source-path.md)) | a `working` status whose message carries a valid entry under `https://agents.vymalo.com/a2a/extensions/steps/v1` in its metadata (`{id, parentId?, kind?, label, state, icon?, detail?, input?, output?}`, [`steps-v1.md`](../../../docs/api/steps-v1.md)) is one `AgentUpdate::Step` envelope with `task_state: Working` instead of a status (the message text is the label for a client that ignores the extension and is not logged). The ids carry the task id (`<task>/<id>`, the parent likewise), so they are unique within the thread. An entry that does not validate (no usable `id` of 1 to 128 bytes without control characters, no `label`, a `state` that is not one of the five, a member of the wrong type, a near-miss URI) or one on a status that is not `working` is ignored: the status is read as plain A2A (fail closed). An unknown `kind` is a tool. Keys: `a2a:<task>:step:<id>:<message id>` (`Turn(<task>:step:<id>:<state>)` without a message id); a snapshot gives the same keys as the stream. The response is data whether or not the request activated the extension. `input` (an object) and `output` (`{text, truncated?, bytes?, error?}`) are read **leniently** ([ADR 0030](../../../docs/decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)): one that is not that shape (an `input` that is a string, an `output` with no string `text`) is dropped and the step is kept, an optional member of an output with the wrong type is read as absent; cutting and redaction are the core's |
| Streamed text (`text-stream/v1`, [ADR 0027](../../../docs/decisions/0027-live-text-relayed-not-stored.md), [`text-stream-v1.md`](../../../docs/api/text-stream-v1.md)) | an artifact update whose artifact carries `{offset}` under the extension's URI in its metadata (stream id of 1 to 128 printable bytes as `artifactId`, exactly one text part, a non-negative whole number for the offset, **read as a float as well as an integer**: A2A's metadata is a protobuf `Struct` and the SDK hands `10` back as `10.0`) is a **chunk of a reply being written**: one envelope with `live: Some(LiveChunk { message_id, offset, text, end })`, no update, no task state, never assembled and never an artifact (`end` is `Last` on `lastChunk`, `Abandoned` when the entry says `abandoned: true`, with or without `lastChunk`); an entry that does not validate leaves the update the plain artifact chunk it was. A held-back artifact is emitted before the chunk, as before any other event. A status message whose metadata names a stream (`{streamId}`, 1 to 128 printable bytes) and whose text is not blank maps to `AgentUpdate::Message { message_id: <stream id>, text, is_final: true }` under `Task("a2a:msg:<stream id>")` **first**, then the status as usual, except that a `working` status carries no `detail` (its words are the message); a turn-ending status keeps its `detail`, which the interrupt and the verifier read. A status that is a step is a step and its marker is ignored. A `Task` (a poll, the first frame of a stream) never shows a chunk: an artifact that carries the entry is skipped. A chunk's key is `Turn("T:live:S:o")`, never stored |
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
| the whole text of stream `S` (a status message that names it) | `Task("a2a:msg:S")` |

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

Unit tests in `src/lib.rs`, offline: a step's `input` and `output` read and, when badly said, dropped alone with the step kept (every wrong shape; an agent that sends neither), streamed text (a chunk is a live piece never applied, assembled or an artifact, its end, a whole offset written as `10.0`, a held-back artifact ended by a chunk, every entry that does not validate left the plain artifact it was, only the exact URI, a task never showing a chunk, a marked status as the message then a status without words for `working` and with them for the turn-ending states, a marker that does not validate, a step winning over a marker, a poll and the stream under the same keys, a stream stated twice as one key); steps (a step with ids made unique by the task, no second `working` for its text, only the required members required and an unknown kind a tool, every member that breaks the entry, a step on any state but `working`, near-miss URIs, a message without an id, a surface in the message of a step, keys equal on a poll and on the stream, the revision of the event with the step); every state, keys, chunked and whole
artifacts, held-back artifacts, messages, snapshots and revisions; and A2UI parts in messages, status messages and artifacts (one envelope per part, the JSON never printed as an artifact, order before the status, keys equal on a poll and on the stream, both spellings of the media type, refusal of malformed, oversized, wrong-content and unsupported-version parts, a refusal not stopping the next part). They moved
here unchanged from `orch-agent-a2a`; the adapter's behaviour through the
mapping stays covered by its `tests/against_fake_agent.rs` and
`tests/conformance.rs`.

## See also

[`orch-agent-a2a`](../agent-a2a/README.md),
[`orch-ports`](../ports/README.md), [`orch-core`](../core/README.md).
