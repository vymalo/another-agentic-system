# ADR 0073 — App Factory board outside the orchestrator, model selection by pinned agent containers, and container-only MVP runtime

- **Status:** proposed (2026-11-18). Decides the orchestration boundary and container-only runtime for the mobile-app and cloud-backend factory ([app-factory plan](../plans/app-factory-mvp-2026-12.md)). Does not amend invariants 1 or 3 of [`CLAUDE.md`](../../CLAUDE.md); keeps [`ADR 0063`](0063-an-openai-compatible-model-endpoint-per-thread-on-the-orchestrator.md), Kubernetes operator integration ([`another-agentic-platform`](https://github.com/vymalo/another-agentic-platform)), and `another-adam-rs` [`ADR 0045`](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0045-a-subagent-can-run-in-the-background-as-a-session.md) slices 3–8 deferred until after the initial release.

## Context

The App Factory needs four user-facing capabilities on top of `another-agentic-system`, running **strictly on containers (Docker / OCI + Compose + Caddy)** with no Kubernetes cluster required for the MVP:

1. **A Kanban board** tracking generated apps, feature cards, git branches (`feat/<slug>`), pull request URLs, container deployment health, and Google Play / Apple TestFlight build status.
2. **Isolated feature chats**, one thread per Kanban card with no cross-feature context bleed.
3. **Model choice per feature card** (frontier reasoning vs fast coding vs lighter utility).
4. **Specialist subagent execution** (schema/backend, mobile UI, container/store ops) visible in the chat activity panel.

Three constraints in this repository shape the design (*verified 2026-11-18 against this repository and `another-adam-rs`*):

- **Orchestrator invariants.** [`CLAUDE.md`](../../CLAUDE.md) invariant 1 ([ADR 0007](0007-protocol-only-dependencies.md)) and invariant 3 ([ADR 0001](0001-rust-state-machine-on-postgres.md)) keep the orchestrator protocol-only over one event log; [ADR 0064](0064-the-person-reaches-the-agent-and-links-their-github.md) states explicitly that *"the orchestrator is never told what a repository is"*. Storing `projects`, `work_items`, `branch_name`, `pr_url`, `deploy_status` and `store_status` in `orch-core` and `orch-store-postgres` would couple the orchestrator to Git, CI, containers and mobile app stores.
- **Unbuilt protocol extensions.** [`docs/api/model-relay-v1.md`](../api/model-relay-v1.md) ([ADR 0063](0063-an-openai-compatible-model-endpoint-per-thread-on-the-orchestrator.md)) has *"Nothing is built"* in either the orchestrator or `another-adam-rs`, and [ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md) currently lists per-person model pickers under *"What stays out"*. Likewise, `another-adam-rs` [ADR 0045](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0045-a-subagent-can-run-in-the-background-as-a-session.md) has slices 1–2 merged (`adam-llm-agent/src/sessions.rs`), while slices 3–8 (mid-turn progress streaming, limits, `child-sessions/v1` extension) are not built. Meanwhile, nested A2A `ask_agent` steps ([ADR 0025](0025-nested-steps-events-carry-their-source-path.md), [ADR 0026](0026-agent-mentions-as-structured-references.md)) and folder subagents ([ADR 0050](0050-the-chat-has-sub-agents.md)) are already built and rendered in `web/`.
- **Container-first execution already exists.** [`compose.yaml`](../../compose.yaml) and [`compose.live.yaml`](../../compose.live.yaml) already run the orchestrator, web UI, Caddy edge, Postgres, `adam-coder`, `github-mcp`, and folder agents (`adam-agent`) as plain containers without Kubernetes.

## Decision

1. **Keep Kanban and app metadata in a separate `factory-board` container service.**
   - A standalone Rust service (`factory-board`, built with CrateStack + Authkestra JWT resource validation) owns `Project`, `BoardCard`, store pre-flight status, git branch, PR status, container rollout status and store release status, and links each card to an orchestrator `threadId`.
   - `factory-board` interacts with `another-agentic-system` solely through its documented HTTP/AG-UI APIs and receives GitHub/CI/deploy/store webhooks without altering `orch-core` or `orch-store-postgres`.
   - When configured, `web/` renders `/projects` and `/projects/[id]` against `factory-board` and opens the orchestrator's existing `ChatShell` (`threadId = card.threadId`) as a slide-over isolated feature chat.

2. **Run both the factory and generated cloud backends as plain containers (no Kubernetes for MVP).**
   - The factory runs as a Docker Compose stack (`compose.factory.yaml` extending `compose.yaml` + `compose.live.yaml`, swapping `mock-oidc` for a real OIDC issuer behind Caddy TLS).
   - Generated app backends ship a pure-Rust OCI container (`apps/server/Dockerfile`) + `deploy/compose/` (`app-server` + `postgres` + Caddy HTTPS) deployed from CI via OCI registry push + `docker compose pull && docker compose up -d --wait`. Kubernetes (`another-agentic-platform` / Helm / CNPG) stays deferred post-MVP.

3. **Deliver model choices via model-pinned agent containers, not `model-relay/v1`.**
   - Run distinct named agent containers (`adam-architect` pinned to a frontier reasoning model, `adam-coder-fast` pinned to a coding model, `adam-coder-light` pinned to a smaller model) registered in `AGENTS_FILE`.
   - Note: [ADR 0049](0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md) `aliases` resolve legacy names onto one canonical agent and are **not** used for distinct models; each model tier is its own agent id in `GET /api/agents`.
   - Each Kanban card stores its chosen `agentId` and can fork/continue onto another model-pinned agent using existing thread continuation ([ADR 0029](0029-forking-a-thread-copies-its-log.md)). [`ADR 0063`](0063-an-openai-compatible-model-endpoint-per-thread-on-the-orchestrator.md) stays *proposed*.

4. **Deliver subagent collaboration with existing A2A `ask_agent` + `ADR 0050` folders.**
   - Specialist agents (`architect`, `cratestack-backend`, `fespalier-mobile`, `cloud-store-ops`) run via existing A2A `@mentions` ([ADR 0026](0026-agent-mentions-as-structured-references.md)) and `ADR 0050` folder subagents + already-merged `SessionTools`. Nested steps render via existing `ADR 0025` support; `another-adam-rs` ADR 0045 slices 3, 4 and 6 wait for a follow-up milestone.

5. **Gate mutating cloud/store MCP actions behind human step-up approval.**
   - By default, generated app repositories deploy containers and push to Play Internal / TestFlight via GitHub Actions on PR merge to `main`.
   - Any MCP tool that triggers a container rollout or store release (`factory-mcp`) uses `authkestra-mcp`'s `HumanApproved` tier (RFC 8785 JCS parameter binding + WebAuthn/TOTP step-up), never unattended `auto` execution.

```mermaid
sequenceDiagram
  autonumber
  actor U as Person
  participant W as Web UI (/projects)
  participant FB as factory-board
  participant O as Orchestrator
  participant A as Model-pinned Agent Container (A2A)

  U->>W: Create card & pick model agent + tool preset
  W->>FB: Create card (agentId, branch feat/slug)
  U->>W: Open card chat & send prompt
  W->>FB: Ensure card threadId
  FB->>O: Create isolated thread (seeded context)
  W->>O: POST /agui/agents/{agentId} (threadId, toolServers)
  O->>A: A2A message (thread-tools/v1, access-tier/v1)
  A-->>O: Nested steps (ADR 0025 / 0050) + PR artifact
  O-->>W: AG-UI stream
  U->>FB: Merge PR on GitHub -> webhook updates card status
```

```mermaid
stateDiagram-v2
  [*] --> Backlog: Card created
  Backlog --> SpecAndPlan: Architect thread started
  SpecAndPlan --> InProgress: Spec confirmed / coder working
  InProgress --> Verifying: PR opened (gates running)
  Verifying --> InProgress: Gate or CI failed (rework)
  Verifying --> Shipped: PR merged + Container & Store green
```

## Alternatives considered

- **Add `projects` and `work_items` tables inside `orch-store-postgres` (rejected).** Violates [ADR 0007](0007-protocol-only-dependencies.md) and [ADR 0064](0064-the-person-reaches-the-agent-and-links-their-github.md) by teaching the orchestrator about git branches, PRs, container rollouts and mobile stores.
- **Require Kubernetes (`another-agentic-platform`, Helm, CNPG) for MVP (rejected).** Adds cluster provisioning, CRD operator, and Helm chart complexity when [`compose.yaml`](../../compose.yaml) already runs the entire stack as containers.
- **Build `model-relay/v1` ([ADR 0063](0063-an-openai-compatible-model-endpoint-per-thread-on-the-orchestrator.md)) and `another-adam-rs` ADR 0045 slices 3/4/6 immediately (deferred).** Adds substantial cross-repo protocol surface.

## Consequences

- **Easier:** `another-agentic-system` keeps invariants 1, 3 and 5 intact with zero new database migrations or protocol extensions required in the orchestrator; container-only deployment eliminates Kubernetes/Helm/CNPG operational overhead for MVP; `factory-board` dogfoods `cratestack-pg` + `authkestra`; per-card model choice works immediately through `GET /api/agents`.
- **Harder:** Deployment includes one extra companion container (`factory-board`) when factory mode is enabled, and model choices are coarse per-agent containers rather than arbitrary model IDs until `ADR 0063` is built. Without Kubernetes `runPods` (`M1`), coding workspaces share a trusted host container runtime with per-thread git worktree isolation.
- **Required elsewhere:** The App Factory plan ([`docs/plans/app-factory-mvp-2026-12.md`](../plans/app-factory-mvp-2026-12.md)), the `vymalo/app-factory-blueprint` repository, and optional factory routes in `web/`.
