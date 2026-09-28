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
| 9 | Workspace image refactor of vymalo/openhand-images. |
