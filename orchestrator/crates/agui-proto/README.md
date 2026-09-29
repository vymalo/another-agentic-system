# orch-agui-proto

The AG-UI 1.0 wire protocol as Rust types, and nothing else: no orchestrator
crate, no agent host, no AG-UI SDK. It is the vocabulary that the AG-UI
projection of the event log (`orch-agui-projection`, and `orch-surface-agui` after it) speaks, kept
apart so it can be checked against the protocol's own schema, and published or
moved later. See [ADR 0004](../../../docs/decisions/0004-closed-enums-over-dyn-registry.md)
(protocols are closed enums) and
[ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md).

## What it holds

- `Event`: one closed `#[serde(tag = "type")]` enum over all **31** 1.0 event
  types, one struct per type named as in the schema (`RunStartedEvent`, ...),
  plus `EventType` (`EventType::ALL`, `as_str`) and `BaseFields` (the
  `timestamp`, `rawEvent` and `metadata` envelope).
- `RunAgentInput`, `Message` (seven roles), `ContentPart`, `PartSource`,
  `Tool`, `Context`, `ResumeEntry`, `Interrupt`, `RunFinishedOutcome`,
  `SubagentFinishedOutcome`, `TokenUsage`, `JsonPatchOperation` and a
  validated `JsonPointer`.
- `AgentCapabilities` (identity, transport, human-in-the-loop, multi-agent, `custom`), the subset of
  the capabilities document a producer of ours declares; every member optional, "not declared" is
  not "unsupported".
- One newtype per id (`ThreadId`, `RunId`, `MessageId`, `ToolCallId`,
  `SubagentRunId`, `InterruptId`), all plain strings on the wire.
- `SCHEMA_1_0` (the vendored schema text) and `PROTOCOL_VERSION = "1.0"`.
- Feature `testkit`: the conformance oracle (below).

## Rules the types keep

- **Absent means absent.** Every optional member is `Option<_>` with
  `skip_serializing_if`; `null` is never written for a missing value.
- **Time is an `i64` of milliseconds**, never `f64`.
- **Outbound is strict, inbound is lenient.** Serialising writes only declared
  members. `RunAgentInput::parse` drops undeclared members and returns their
  paths in `ParsedInput::dropped` (the caller logs the warning); a malformed
  input (missing required member, wrong type, unknown message role) is an
  `InputError`, to be rejected before `RUN_STARTED`.
- **Closed enums.** An event `type` this crate does not know does not
  deserialise. A consumer that must tolerate newer minor versions skips unknown
  events before handing them to serde.
- Members the schema types as "any JSON value" are `serde_json::Value`. Where
  the schema says `not: null`, they are `Option<Value>` and a JSON `null`
  reads as absent.

## API sketch

```rust
use orch_agui_proto::*;

let started: Event = RunStartedEvent::new("thread-1", "run-1").into();
let text = TextMessageContentEvent::new("evt-1", "hello");
let done: Event = RunFinishedEvent::new("thread-1", "run-1", RunFinishedOutcome::success())
    .into();
let wire = serde_json::to_string(&done)?; // {"type":"RUN_FINISHED","threadId":...}

let parsed = RunAgentInput::parse(request_body)?; // Err before the stream starts
for path in &parsed.dropped { /* warn: dropped member */ }
let input: RunAgentInput = parsed.input;
```

## Conformance testkit

Enable `testkit` as a dev-dependency feature in any crate that emits AG-UI
events:

```toml
[dev-dependencies]
orch-agui-proto = { workspace = true, features = ["testkit"] }
```

- `testkit::assert_conforms(&Event)` and
  `testkit::assert_input_conforms(&RunAgentInput)` serialise the value and
  validate it against the vendored schema (`#/$defs/Event`,
  `#/$defs/RunAgentInput`), panicking with the violations and the JSON.
- `assert_json_conforms` / `assert_input_json_conforms` do the same for a
  `serde_json::Value` (goldens); `event_errors` / `input_errors` return the
  messages instead of panicking.
- `assert_capabilities_conform(&AgentCapabilities)`, `assert_capabilities_json_conforms` and
  `capabilities_errors` do the same for `#/$defs/AgentCapabilities`.
- The schema checks **structure**. Ordering and lifecycle (runs balanced,
  nothing open at a terminal event) are behavioural and are checked elsewhere.
- The schema is strict on purpose: do not use it to validate inbound requests.
  `RunAgentInput::parse` is the receive path.
- Validation uses `jsonschema` 0.58 (already in the workspace lock file), with
  no network access: the schema's `$id` is its own base, and every `$ref` in
  it is local.

## The vendored schema

| | |
|---|---|
| File | [`schema/ag-ui-1.0.schema.json`](schema/ag-ui-1.0.schema.json), byte for byte as served |
| Source | https://ag-ui.com/spec/1.0/schema.json (a 301 to https://docs.ag-ui.com/spec/1.0/schema.json; both serve the same file) |
| Fetched | 2026-09-29 |
| sha256 | `4b5c93226838a0e72d88e6c5df20633c686c49fcb75d9be2815c6cbf9e48e71a` |
| `$id` | `https://ag-ui.com/spec/1.0/schema.json` (JSON Schema 2020-12, 98 definitions, 31 event types) |
| Status | *verified* 2026-09-29: fetched from the source URL above, and identical to the copy in the research download taken the same day |

The spec's schema page says the address "serves the file directly: no redirect
stands between a tool and the schema". That is not true today: the address
answers 301. Fetch with `curl -L`.

To update: fetch the new file, replace it, put its sha256 and the date here in
the same commit (the test `readme_records_the_sha256_of_the_vendored_schema`
fails otherwise), and review the type changes the diff calls for. A new spec
version is a new file (`ag-ui-1.1.schema.json`), not an edit.

## How it is tested

`cargo test -p orch-agui-proto`:

- `tests/wire.rs`: literal wire JSON for every event type (all optional members
  set, and required members only) and every message role, parsed and serialised
  back to the identical JSON; the `EventType` list equals the schema's
  `EventType` enum, in order; unknown types and missing members are errors.
- `tests/conformance.rs`: every fixture and every constructor validates
  against the schema; the **absent, not null** check (a required-only value
  writes exactly the schema's `required` members); negative cases prove the
  oracle can say no.
- `tests/input.rs`: unknown-member stripping with the reported paths, open
  objects kept whole, malformed inputs rejected.
- `tests/props.rs`: proptest generators for every event type and for
  `RunAgentInput` check round trips through text and `Value`, schema
  conformance, and that unknown members never change the typed view.
- `tests/capabilities.rs`: the `AgentCapabilities` wire shape with every member set, round trip, an
  empty document, and that the oracle says no (an undeclared member, `subagents` as a flag).
- `tests/schema.rs`: the README's sha256 matches the vendored file.
