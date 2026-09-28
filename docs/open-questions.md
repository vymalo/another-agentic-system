# Open questions

Things we have not verified or decided. Each should be closed by an experiment
or an ADR, not by assumption.

| # | Question | Why it matters | How to close it |
|---|---|---|---|
| 1 | **kagent 0.x or 1.x?** `v1.0.0-alpha5` shipped 2026-09-27 on a new `v1alpha3` API (`Agent`, `ModelConfig`, `RemoteMCPServer`, `SandboxTemplate`); the docs still call 0.x "current". | Building on an alpha API means churn; building on 0.x means a migration later. | Read the 1.x changelog; prototype one agent on each. |
| 2 | **Can kagent host our sandbox workers?** 1.x has a `SandboxTemplate` resource whose image must be pinned by `sha256` digest (verified in `sandboxtemplate_types.go`). | Could replace a custom sandbox launcher. | Prototype an opencode worker as a kagent sandbox. |
| 3 | **opencode A2A wrapper** — opencode speaks its own HTTP API (`opencode serve`, default `127.0.0.1:4096`, basic auth via `OPENCODE_SERVER_PASSWORD`), not A2A. | This wrapper is real custom code and a core dependency. | Spike: A2A server (a2a-lf) → `opencode serve` session → stream events → push branch. |
| 4 | **a2a-lf maturity.** The official Rust SDK is new. | The orchestrator depends on it for both directions. | Spike against a kagent agent. |
| 5 | **Worker auth to models:** gateway API keys (AISIX) or subscription logins (Claude/Codex)? | Subscription logins are per-person and live on disk; gateway keys are central and budgetable. | Decide per worker type; default to AISIX keys. |
| 6 | **Budgets:** attempts, tokens, wall clock per job and per step. | Multi-agent loops can burn tokens silently. | ADR once AISIX usage data exists. |
| 7 | **Sandbox isolation:** NetworkPolicy, no sudo, separate namespace/nodes from CI runners. | Agents run arbitrary code (see lessons #12). | Threat model before step 2 of the MVP. |
| 8 | **Where does the verifier run?** Real CI (GitHub Actions on the ARC runners) or checks inside the sandbox? | CI is the source of truth but slower; in-sandbox is fast but can diverge. | Start with in-sandbox checks, gate the PR on real CI. |
| 9 | **Workspace image.** Refactor `vymalo/openhand-images` so the toolchain recipe is a shared script with two images (`agent-canvas`, `workspace`). | Workers need the toolchains without Agent Canvas. | Do it with MVP step 2. |
