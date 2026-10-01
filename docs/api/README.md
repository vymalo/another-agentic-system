# API contracts

What the orchestrator says to the outside, one page per contract. The event log is the source of truth;
every page below is a view of it or of a decision about it ([`../architecture.md`](../architecture.md)).

| Page | What it is | Status |
|---|---|---|
| [`chat-api.yaml`](chat-api.yaml) | OpenAPI 3.1: the resource API (agents, threads, export, cancel, health) and the AG-UI operations | Built |
| [`agui.md`](agui.md) | The AG-UI 1.0 binding: run, connect and capabilities, the log-to-AG-UI mapping, the `vymalo.*` schemas, A2UI | Built |
| [`webhooks.md`](webhooks.md) | CI results by webhook: the generic signed shape and the GitHub adapter | Built |
| [`ui-catalog-v1.md`](ui-catalog-v1.md) | The web's component catalog, sent to agents (A2A extension `ui-catalog/v1`) | Contract accepted; the orchestrator's handshake and the refetch (`get_ui_catalog` on the thread tools) are built |
| [`thread-tools-v1.md`](thread-tools-v1.md) | A per-thread MCP endpoint for agents, with the HMAC token that opens it (A2A extension `thread-tools/v1`) | Contract accepted; built (the token, the endpoint, `get_ui_catalog` and the grant in the A2A message), apart from the tools of slices 8 and 10 |
| [`steps-v1.md`](steps-v1.md) | Nested steps: an agent's tool calls and its sub-agents' work reported as a tree (A2A extension `steps/v1`) | Contract accepted; built (the `agent_step` event and its coalescing, the AG-UI subagents and `vymalo.step` activities, the adapter), apart from the web's tree |
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
| Thread tools | `thread-tools/v1` | [`thread-tools-v1.md`](thread-tools-v1.md) (accepted; built, apart from the tools of slices 8 and 10) | ADR 0023, [0024](../decisions/0024-mcp-tools-attached-per-conversation.md), [0026](../decisions/0026-agent-mentions-as-structured-references.md) |
| Steps | `steps/v1` | [`steps-v1.md`](steps-v1.md) (accepted; built, apart from the web's tree) | [ADR 0025](../decisions/0025-nested-steps-events-carry-their-source-path.md) |
| Mentions | `mentions/v1` | A page here, written with MVP slice 10 | ADR 0026 |

The orchestrator reads the extensions of a live card into one closed set, and the AG-UI capabilities document lists
each listed one under `custom` (see [`agui.md`](agui.md#capabilities-document)), so the web can flag an agent that
lacks one before the person sends a message that needs it.
