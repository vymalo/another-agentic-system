# API contracts

What the orchestrator says to the outside, one page per contract. The event log is the source of truth;
every page below is a view of it or of a decision about it ([`../architecture.md`](../architecture.md)).

| Page | What it is | Status |
|---|---|---|
| [`chat-api.yaml`](chat-api.yaml) | OpenAPI 3.1: the resource API (agents, threads, export, cancel, health) and the AG-UI operations | Built |
| [`agui.md`](agui.md) | The AG-UI 1.0 binding: run, connect and capabilities, the log-to-AG-UI mapping, the `vymalo.*` schemas, A2UI | Built |
| [`webhooks.md`](webhooks.md) | CI results by webhook: the generic signed shape and the GitHub adapter | Built |
| [`ui-catalog-v1.md`](ui-catalog-v1.md) | The web's component catalog, sent to agents (A2A extension `ui-catalog/v1`) | Contract accepted; the orchestrator's handshake and the refetch (`get_ui_catalog` on the thread tools) are built |
| [`thread-tools-v1.md`](thread-tools-v1.md) | A per-thread MCP endpoint for agents, with the HMAC token that opens it (A2A extension `thread-tools/v1`): the UI catalog, the answer announcement, the `attached` servers and their relayed tools, and `ask_agent` | Contract accepted; built (the token, the endpoint, `get_ui_catalog`, `turn_output` and the grant in the A2A message). Built 2026-10-02: `attached` and the relay (`_meta` `reportsStep` and `timeoutSecs`, the step of each call). Written, **not built**: `ask_agent` with its limits ([ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md), [ADR 0026](../decisions/0026-agent-mentions-as-structured-references.md)) |
| [`steps-v1.md`](steps-v1.md) | Nested steps: an agent's tool calls and its sub-agents' work reported as a tree (A2A extension `steps/v1`) | Contract accepted; built (the `agent_step` event and its coalescing, the AG-UI subagents and `vymalo.step` activities, the adapter), apart from the web's tree. Revised 2026-10-02: a step may carry `input` and `output`, cut, redacted and budgeted ([ADR 0030](../decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)); built in the orchestrator, not yet drawn by the web nor sent by an agent |
| [`steer-v1.md`](steer-v1.md) | A message to a task that is running, read at the agent's next step (A2A extension `steer/v1`): the card, the activation, the message, the refusals ([ADR 0036](../decisions/0036-sending-while-an-agent-works.md)) | Contract accepted 2026-10-02; orchestrator side built (PR-13) |
| [`mentions-v1.md`](mentions-v1.md) | Agents mentioned in a message as structured references (id, label, UTF-16 offsets), how they are checked and what the addressed agent receives (A2A extension `mentions/v1`) | Contract accepted 2026-10-02; not built (MVP slice 10) |
| [`text-stream-v1.md`](text-stream-v1.md) | An agent's reply streamed as it is written, relayed live and logged once (A2A extension `text-stream/v1`) | Contract accepted; built on the orchestrator's side (the adapter, the relay between processes, the AG-UI live frames) and in the web (drafts) |
| [`config.md`](config.md) | The orchestrator's one YAML configuration file: every key, which environment variable it replaces, which PR builds it; secrets by reference; `GET /api/config`, the web's public subset ([ADR 0034](../decisions/0034-one-yaml-configuration-secrets-by-reference.md), [ADR 0035](../decisions/0035-utility-model-tasks.md)) | Built (plan 10, S9, 2026-10-02): the file, the loader, `--print-config`, [`config.schema.json`](config.schema.json); `GET /api/config` with S18 |
| [`examples/`](examples/README.md) | Golden event transcripts and AG-UI streams that tests pin | Built |

## Optional A2A extensions the orchestrator speaks

Each is optional, in the pattern of [ADR 0008](../decisions/0008-platform-integration-via-a2a-extension.md): an agent
lists it in its card, the orchestrator reads the card on every send (never cached), an agent without it gets plain
A2A, and nothing breaks when it is removed. The URIs follow the release-channels extension of
another-agentic-platform (`docs/extensions/release-channels-v1.md` in that repository):
`https://agents.vymalo.com/a2a/extensions/<name>/v1`, decided on the owner's delegation on 2026-10-01. A breaking
change is a `v2` URI and a new page, not an edit.

| Extension | URI suffix | Contract | Decided in |
|---|---|---|---|
| UI catalog | `ui-catalog/v1` | [`ui-catalog-v1.md`](ui-catalog-v1.md) (accepted; built, the refetch included) | [ADR 0023](../decisions/0023-ui-component-catalog-as-an-a2a-extension.md) |
| Thread tools | `thread-tools/v1` | [`thread-tools-v1.md`](thread-tools-v1.md) (accepted; built, apart from `ask_agent`, whose contract is written) | ADR 0023, [0024](../decisions/0024-mcp-tools-attached-per-conversation.md), [0026](../decisions/0026-agent-mentions-as-structured-references.md) |
| Steps | `steps/v1` | [`steps-v1.md`](steps-v1.md) (accepted; built, apart from the web's tree) | [ADR 0025](../decisions/0025-nested-steps-events-carry-their-source-path.md) |
| Mentions | `mentions/v1` | [`mentions-v1.md`](mentions-v1.md) (accepted 2026-10-02; not built) | ADR 0026 |
| Steer | `steer/v1` | [`steer-v1.md`](steer-v1.md) (accepted 2026-10-02; orchestrator side built in PR-13) | [ADR 0036](../decisions/0036-sending-while-an-agent-works.md) |
| Text stream | `text-stream/v1` | [`text-stream-v1.md`](text-stream-v1.md) (accepted; built in the orchestrator and the web) | [ADR 0027](../decisions/0027-live-text-relayed-not-stored.md) |

The orchestrator reads the extensions of a live card into one closed set, and the AG-UI capabilities document lists
each listed one under `custom` (see [`agui.md`](agui.md#capabilities-document)), so the web can flag an agent that
lacks one before the person sends a message that needs it.
