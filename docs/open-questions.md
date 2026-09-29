# Open questions

Things not yet verified or decided. Each should be closed by an experiment or
an ADR, not by assumption.

## Open

| # | Question | Why it matters | How to close it |
|---|---|---|---|
| 4 | **a2a-lf maturity.** The official A2A Rust SDK ([`a2aproject/a2a-rs`](https://github.com/a2aproject/a2a-rs), crate `a2a-lf`) is new. | The orchestrator depends on it in both directions. | Spike against one A2A agent (ADK-Rust or kagent). |
| 6 | **Budgets:** attempts, wall clock and tokens per job and per step. | Multi-agent loops can burn tokens silently. The orchestrator owns attempts and wall clock; tokens are spent by agents and measured by their gateways. | ADR once gateway usage data (EAIG / AISIX) is reachable. |
| 8 | **Where does verification run?** Real CI (GitHub checks arriving by webhook) or a verifier agent over A2A? | CI is the source of truth but slower; a verifier agent is faster but can diverge. | Gate the PR on real CI; allow a verifier agent for inner loops. |
| 10 | **Release-channels extension v1 stability.** The contract lives in another-agentic-platform (`docs/extensions/release-channels-v1.md`) and is a draft. | ADR 0008 depends on it. | Freeze v1 when the platform's MVP step 5 ships. |
| 11 | **Authentication to agents.** How does the orchestrator authenticate to A2A agents and MCP servers (per agent card's security schemes)? | Protocol-only doesn't mean anonymous. | Support the schemes the first agents declare; secrets via external-secrets. |
| 12 | **Connect binding upstream.** Our `GET /agui/threads/{id}/connect` (replay, tail across runs, `Last-Event-ID`) is an extension; AG-UI 1.0 standardises no resumption ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md)). | Standard clients get run-scoped streams only; if upstream standardises resumption differently, we migrate. | Ship ours; open an upstream discussion on resumable SSE and follow `transport.resumable`. |
| 13 | **Implicit answers.** Is a new user message on a blocked thread without `resume` an answer, or rejected? | The spec lets the producer reject; today's semantics accept. | Accept and document it ([`agui.md`](api/agui.md)); revisit if a client relies on rejection. |
| 14 | **Non-extending partial agent messages.** A partial `agent_message` that does not extend the previous text cannot be an AG-UI delta. | Streaming agents could garble text. | Contract rule: partials are cumulative prefixes; otherwise the A2A adapter starts a new message id. |
| 15 | **`auth_required` resolution.** A user message, or out-of-band credentials? | The interrupt `reason` becomes truthful once `AgentStatus::AuthRequired` exists. | A user message for now; decide with #11. |
| 16 | **Cancelled subagent.** AG-UI 1.0's subagent outcome has no `cancelled` member. | A cancelled delegation is shown as `SUBAGENT_FINISHED{result:{status:"canceled"}}`. | Raise upstream; keep the `result` form until then. |
| 17 | **Producer-initiated runs.** Webhooks and timers open runs with no `RunAgentInput` (`RUN_STARTED` without input). | The spec's "several runs per stream" rule seems to allow it; clients may not expect it. | Confirm with the reference consumer (`verifyEvents`) and react-ag-ui in the spikes. |
| 18 | **`RunAgentInput.context` and frontend `tools`.** | Ignored in the MVP. | Later: forward `context` as A2A message metadata; frontend tools need a model in the browser's loop. |
| 19 | **Body limit on `/agui/agents/*`.** Standard clients POST the whole transcript. | Long threads hit the 1 MiB limit. | 8 MiB on that route plus id reconciliation; document `resumeTranscript: "appended"` for react-ag-ui. |
| 20 | **External programmatic AG-UI clients.** How do they authenticate through the edge? | Browser sessions go through oauth2-proxy; a CLI has no cookie. | oauth2-proxy JWT bearer pass-through; decide when there is a client. |
| 21 | **`@assistant-ui/react-ag-ui` on `@ag-ui/client` 1.0.** It depends on `^0.0.59`; our patches and an `overrides` entry may bridge it. | Patches rot on upgrade; the runtime's `unstable_` APIs may change in a patch release. | Spike: run the pinned runtime on 1.0 via `overrides`; propose the bump and our fixes upstream; drop each patch when a release contains it. |
| 22 | **A2UI 1.0.** The A2A extension URI we detect is `…/a2ui/v0.9.1`; v1.0 is a candidate ([ADR 0013](decisions/0013-a2ui-generative-ui.md)). | A new URI means a card change on the agent side and a detection change here. | Detect both URIs while v1.0 settles; bump in a reviewed PR. |
| 23 | **Explicit default marker (`default: true`) instead of list order?** The default agent is the first `AGENTS_FILE` entry ([ADR 0014](decisions/0014-adam-coder-default-agent-over-a2a.md)). | Reordering a file changes the default silently; a marker is explicit but adds startup errors (none or two defaults). | Keep order until a deployment reorders by accident or a second client needs another default; then an ADR. |

## Closed

| # | Question | Resolution |
|---|---|---|
| 3 | opencode speaks its own API, not A2A — who writes the wrapper? | The agent host's harness: another-agentic-platform runs ADK-Rust (A2A via `adk-server`) driving `opencode acp` over stdio in the same Pod. |

## Moved to another-agentic-platform

These are agent-hosting concerns now that this system is protocol-only (ADR 0007):

| # | Question |
|---|---|
| 1 | kagent 0.x vs 1.x — kagent is now just one optional agent host. |
| 2 | Sandbox hosting for coding workers (`SandboxTemplate` etc.). |
| 5 | Worker model auth (gateway keys vs subscription logins). |
| 7 | Sandbox isolation (NetworkPolicy, no sudo, separation from CI) → platform `SecurityProfile`. |
| 9 | Workspace image refactor of vymalo/another-agentic-images. |
