# another-agentic-system

<img src="web/public/brand/panda.svg" alt="A boring giant panda in three colours, the mark of another-agentic-system" width="96" height="96" align="right">

A protocol-agnostic **orchestration layer** for multi-agent work. You start a
job from a chat — or another system starts one over A2A, MCP or a webhook.
An agent works it (and may ask the other agents you mentioned), the system
verifies it against real checks, and a pull request comes back. You read the
chat surface. Planner-led parallel work and reviewer agents are post-MVP.

> **Status, 2026-10-03: the MVP is complete against its build order** — the orchestrator
> (`orchestrator/`) and the chat surface (`web/`) carry every slice of the [MVP](docs/mvp.md) and every
> requirement of the [vision](docs/vision.md) that is not listed there as post-MVP: a live registry of agents, rich
> answers and questions through the web's own components, tools attached to a conversation, several agents in one
> message by mention, nested steps, workspaces and devcontainers for the coder, writing while an agent works, forks,
> files, roles. **Complete is not accepted.** Everything is proven on mocks (WireMock agents, a scripted model, mock
> GitHub, search, registry and sign-in) by the scenario scripts of [`dev/`](dev/README.md) and by tests; nothing was run
> against a live model, GitHub.com, the platform or a real browser agent (there is none: the browser in the football
> example is a mock), and the owner, who judged the first build "not ready" on 2026-10-01, has not tried this state.
> What is built and how it is proven, requirement by requirement: [vision](docs/vision.md#requirement-by-requirement-2026-10-03);
> what is post-MVP: [MVP](docs/mvp.md#post-mvp); the decision: [ADR 0037](docs/decisions/0037-the-mvp-is-complete-against-its-build-order.md)
> (proposed). What exists, with diagrams: [Architecture: as built](docs/architecture.md#as-built).
> Decisions are recorded as ADRs; what is not yet verified is listed in
> [open questions](docs/open-questions.md).

It hosts no agents. It drives anything that speaks A2A, uses tools over MCP,
reacts to webhooks, and can be driven the same way. Its processes are
stateless; the only durable state is the job ledger and event log (the chat)
in Postgres.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="web/e2e/__screens__/desktop-dark-done-pull-request.png">
  <img src="web/e2e/__screens__/desktop-light-done-pull-request.png" alt="A finished job in the chat. Left: the thread list. Middle: the coder’s answer under the line “9 steps · 4s”, with a code block and a pull request card with a View pull request button. Right: the Activity panel listing the steps, from reading the code to the passing checks and the opened pull request." width="720">
</picture>

*A finished job in the chat surface: the answer and its pull request in the middle, the steps the agent took in the panel on the right, the threads on the left. The screenshot is from the web’s own mock server (`pnpm screens`), not from the compose stack. More screens, phone layouts included: [`web/README.md`](web/README.md#what-it-looks-like).*

## In one picture

```mermaid
flowchart LR
  you((You)) -- browser --> edge[Edge proxy<br/>oauth2-proxy]
  edge -- UI --> cp[Web chat surface<br/>Next.js + assistant-ui]
  edge -- "/api/*, /agui/*" --> orch[Orchestrator<br/>stateless Rust replicas]
  orch <--> db[(Postgres / CNPG<br/>chat · threads · outbox)]
  orch -- A2A --> agents[Agents — any A2A host<br/>another-agentic-platform · kagent · …]
  agents -- push branch --> git[(git → PR)]
  ext[Other systems · MCP clients · webhooks · timers] -- A2A / MCP / HTTP --> orch
  orch -- MCP --> tools[Tools: GitHub, docs, search…]
  orch -- OpenAI-compatible --> gw[Model endpoint<br/>EAIG / Agent Router · AISIX · …]
```

Everything drawn is built (tools are the servers a deployment lists, relayed to agents; the model endpoint is the orchestrator's own, for thread titles and descriptions). The web serves the UI only; the browser talks to the
orchestrator through the edge. Component, request and state diagrams:
[Architecture](docs/architecture.md#as-built).

## Principles

1. **Protocols only** — no agent host, gateway product or SDK is a hard
   dependency. ([ADR 0007](docs/decisions/0007-protocol-only-dependencies.md))
2. **Stateless processes, one event log.**
   ([ADR 0001](docs/decisions/0001-rust-state-machine-on-postgres.md))
3. **Verification over consensus.**
   ([ADR 0002](docs/decisions/0002-verification-over-consensus.md))
4. **git is the artifact.**
   ([ADR 0003](docs/decisions/0003-git-as-durable-state-ephemeral-workers.md))
5. **The orchestrator never sees a protocol** — adapters around a pure core.
   ([orchestrator](docs/orchestrator.md))

## Documents

| Document | What it covers |
|---|---|
| [Architecture](docs/architecture.md) | Components, agent hosts, **as built** (component diagram, a chat turn over AG-UI, thread state, how AG-UI is served), the target job flow and lifecycle, where it runs |
| [Orchestrator](docs/orchestrator.md) | Ports & adapters, the crate dependency graph, event flow (design against built), outbox lifecycle, transition table, core types, data model, live updates (how AG-UI streams come from the log), testing |
| [Orchestrator workspace](orchestrator/README.md) | Running it, configuration, shutdown, error classes; each crate has its own README (role, API, environment, tests) |
| [MVP](docs/mvp.md) | Build order, smallest working loop first, with what is built |
| [API contract](docs/api/chat-api.yaml) | OpenAPI 3.1: the resource API (agents, threads, cancel, health), the AG-UI operations (the legacy REST interaction endpoints were removed on 2026-09-30) |
| [AG-UI binding](docs/api/agui.md) | How the orchestrator speaks AG-UI 1.0: run and connect endpoints, log-to-AG-UI mapping, `vymalo.*` schemas |
| [Webhooks](docs/api/webhooks.md) | CI results by webhook: the generic signed shape (built) and the GitHub adapter (planned): headers, HMAC, body, conclusions, response codes, a worked signature and known-answer vectors |
| [API index](docs/api/README.md) | What is in `docs/api/`, and the optional A2A extensions the orchestrator speaks: [`ui-catalog/v1`](docs/api/ui-catalog-v1.md) (the web's component catalog, sent to agents) and [`thread-tools/v1`](docs/api/thread-tools-v1.md) (a per-thread MCP endpoint for agents, with an HMAC token); contracts accepted 2026-10-01 and built (the catalog handshake, the thread tools with their token and `get_ui_catalog`), except what [`thread-tools/v1`](docs/api/thread-tools-v1.md) marks as later slices |
| [Deployment chart](deploy/chart/README.md) | A real deployment on Kubernetes ([ADR 0041](docs/decisions/0041-deployed-with-helm-on-kubernetes-secrets-by-externalsecret.md)): the Helm chart, the AWS secret's properties and which pod reads each, the owner's manual steps, the Keycloak client ([`deploy/keycloak`](deploy/keycloak/README.md)) |
| [Open questions](docs/open-questions.md) | Open, closed, and moved to the platform |
| [Lessons from Agent Canvas](docs/lessons-from-agent-canvas.md) | What running OpenHands Agent Canvas taught us, as requirements |

### Decisions

| ADR | Decision |
|---|---|
| [0001](docs/decisions/0001-rust-state-machine-on-postgres.md) | Orchestrator is a Rust state machine on Postgres (not eve, not Restate) |
| [0002](docs/decisions/0002-verification-over-consensus.md) | Verification over consensus |
| [0003](docs/decisions/0003-git-as-durable-state-ephemeral-workers.md) | git is the durable artifact; workers are ephemeral |
| [0004](docs/decisions/0004-closed-enums-over-dyn-registry.md) | Protocols as closed enums, not a dynamic adapter registry |
| [0005](docs/decisions/0005-openai-compatible-model-endpoint.md) | Model access through any OpenAI-compatible endpoint. *Amended 2026-10-01: the orchestrator's first model call, thread titles, through the `ChatModel` port; extended by 0035* |
| [0006](docs/decisions/0006-assistant-ui-external-store.md) | Chat surface: Next.js + assistant-ui (an external store at first, now the AG-UI runtime per 0012) |
| [0007](docs/decisions/0007-protocol-only-dependencies.md) | Protocol-only dependencies: an agnostic orchestration layer |
| [0008](docs/decisions/0008-platform-integration-via-a2a-extension.md) | Optional another-agentic-platform integration via an A2A extension |
| [0009](docs/decisions/0009-swappable-implementations-at-build-time.md) | Swappable implementations, selected at build time |
| [0011](docs/decisions/0011-web-shadcn-tailwind-feature-layout.md) | Chat surface: shadcn/ui on Tailwind v4, kebab-case feature layout |
| [0012](docs/decisions/0012-ag-ui-user-facing-protocol.md) | AG-UI 1.0 as the user-facing protocol; REST kept for resources; surfaces mounted by configuration |
| [0013](docs/decisions/0013-a2ui-generative-ui.md) | A2UI for generative UI, end to end over A2A and AG-UI |
| [0014](docs/decisions/0014-adam-coder-default-agent-over-a2a.md) | adam-coder is the default agent (first `AGENTS_FILE` entry), over plain A2A |
| [0015](docs/decisions/0015-control-plane-and-workers-on-adam-rs.md) | Control plane and workers on adam-rs (`Role` enum, git rev); in-process agents behind a feature; amends 0001 and 0007 |
| [0016](docs/decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md) | Inbox, timers and the job ledger on the thread (`threads.job`); unsolicited machine input only. *Slices 2 and 5 built* |
| [0017](docs/decisions/0017-ci-results-by-webhook.md) | CI results by webhook: a GitHub adapter and a generic signed shape. *Generic route built (slice 6); GitHub adapter planned* |
| [0018](docs/decisions/0018-verification-gate-and-rework-loop.md) | Configurable verification gate (CI, agent checks, verifier agent) and a bounded rework loop; refines 0002. *Built for the agent's own checks (slices 2, 3) and the verifier agent (slice 10); CI built (slice 6)* |
| [0019](docs/decisions/0019-mcp-server-over-streamable-http.md) | MCP server over streamable HTTP, bearer tokens first, OIDC later; bypasses the inbox. *Tools, bearer tokens and `wait_for_job` built; OIDC planned* |
| [0020](docs/decisions/0020-a-thread-is-a-conversation.md) | A thread is a conversation: a message on a finished thread starts its next job (`Job.number`, `job_started`); amends 0012, 0016, 0018, 0019 |
| [0021](docs/decisions/0021-context-across-a2a-tasks.md) | Context across A2A tasks: same context, `referenceTaskIds` of the previous task on every new task of a thread, never for the verifier |
| [0022](docs/decisions/0022-platform-provisions-agents-system-discovers-them.md) | The platform provisions A2A agents and the system discovers them through an `AgentRegistry` port; the platform knows nothing about the UI. *Accepted 2026-10-01 (owner's delegation)* |
| [0023](docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md) | A UI component catalog on A2UI catalogs, as an optional A2A extension, sent at conversation start, with a refetch seam. *Accepted 2026-10-01 (owner's delegation)* |
| [0024](docs/decisions/0024-mcp-tools-attached-per-conversation.md) | MCP tools attached per conversation from the UI and relayed to agents by the orchestrator, which holds the credentials. *Accepted 2026-10-01 (owner's delegation)* |
| [0025](docs/decisions/0025-nested-steps-events-carry-their-source-path.md) | Nested steps: events carry their source path; a collapsible tree in the web. *Accepted 2026-10-01 (owner's delegation)* |
| [0026](docs/decisions/0026-agent-mentions-as-structured-references.md) | Agent mentions as structured references; the addressed agent coordinates the mentioned agents with `ask_agent`. *Accepted 2026-10-01 (owner's delegation)* |
| [0027](docs/decisions/0027-live-text-relayed-not-stored.md) | Live text is relayed, not stored: an agent's words while it writes them go over the `Wakeup` port (`orch_live`) to every process and are shown by an overlay; the log keeps only the final message; amends 0012. *Accepted 2026-10-01 (owner's delegation); built: the port, the overlay, the relay and `text-stream/v1`* |
| [0028](docs/decisions/0028-devcontainer-json-is-the-workspace-environment-contract.md) | A repository's `devcontainer.json` is its work environment: the coder builds it with the official CLI against a rootless Podman service; the first repository of a workspace decides and the `workspace` image is the default; Kubernetes waits for the platform's sandbox provider. *Accepted 2026-10-01 (owner's decision); built 2026-10-02 (MVP slice 7b)* |
| [0029](docs/decisions/0029-forking-a-thread-copies-its-log.md) | Forking a thread copies its log: a fork is a new thread that starts with the parent's events up to a cut, then `thread_forked`; it has its own A2A context and its agent is told the conversation as a fenced transcript; an edit is a fork with its message and a sibling of the thread it came from; the parent's title and title ledger are kept. *Accepted 2026-10-01 (owner's request, defaults on delegation); built: the core, the store (migration 0010), the API, the AG-UI marker, the transcript the agent of a fork is told, and the web (fork from any turn, edit into a branch)* |
| [0030](docs/decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md) | A step carries its input and output: `steps/v1` gains `input` (with the start) and `output` (with the end), cut (4 KiB in, 8 KiB out, 2 MiB per job), redacted in the agent and in the core (a filter, not a guarantee), recorded by default with a switch (`ORCH_STEPS_RECORD_IO`), parsed leniently; relayed calls record theirs too; amends 0025 and 0024. *Accepted 2026-10-02 (owner's answer); built: the core, the adapter, the AG-UI activity, the contract, the web (a step opens onto its input and output) and the agent side (adam-rs `d56dd94`)* |
| [0031](docs/decisions/0031-working-text-and-the-turns-answer.md) | Working text and the turn's answer: the adapter marks the text it reads by the status it came on (`purpose`: `working` on a `working` status, `answer` on `completed`, `input_required`, `auth_required`; a plain A2A `Message` is not marked), `AgentMessageData` carries it (and the reserved `via`), the AG-UI `START` says it in `vymalo.purpose`, a live message that proves to be working text says so on its `END`; no `REASONING_*`; an unmarked text is the screen's fallback. Extends 0012 and 0027. *Accepted 2026-10-02 (owner's answer: the rule first, the `turn_output` tool after); amended 2026-10-02: the `turn_output` thread tool is built (`Input::Answer`, `via: turn_output`, the id `out-<jti>-<n>`; once a turn has an announced answer everything else it says is working text, the status words included; a later call replaces the answer by the rule that the answer of a turn is the last message marked `answer`); built: the adapter, the core, the projection and the overlay, the tool, the contract and the web's rendering* |
| [0032](docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md) | Files from agents live in an artifact store; the log keeps references: the port `ArtifactStore` (`put` idempotent by content, `get` as a stream, `delete`) keyed `threads/<thread>/<sha256>`, a directory store for development and one node and an S3 store (`object_store`) for production, chosen by `artifacts.store`; the hand-over is an A2A file part (`raw`, never base64 through the model), ingested on the worker before the commit with a size cap, a sniffed type and an `error` when a file cannot be kept; served behind the identity layer with `nosniff`, a sandboxing CSP and an immutable cache, SVG sanitized, the catalog's `Image` from thread files only; amends the wording of invariant 3 and ADR 0001, extends 0013. Keys: [`docs/api/config.md`](docs/api/config.md). *Accepted 2026-10-02 (owner's question); built: the port, the testkit, both stores, the configuration and the wiring (S10), the ingest, serving and the sanitizer (S11), the web (S12) and `dev/artifact-e2e.sh` with adam's `share_file` (S13); not built: deleting files (open question 46)* |
| [0033](docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md) | The orchestrator is an OAuth2 resource server (an `Authenticator` port; `orch-auth-jwt` validates bearer tokens against the issuer's JWKS, RS256/RS384/ES256/EdDSA, `auth.mode` `proxy_header` \| `jwt` \| `jwt_or_proxy_header`); roles map to permissions, enforced by a pure `authz`, admins read any thread and act on their own (*reversed by 0039, 2026-10-03: no role reads another person's thread*); amends 0012, closes question 20. *Port, JWT and header authenticators built (S14); roles, enforcement, `/api/me`, the administrators' listing and bounded streams built (S15); the web reads `/api/me`: read-only threads, the agent picker, the administrators' list, no access and the sign-in redirect (S17); the dev stack with a mock issuer and a real oauth2-proxy (S16)* |
| [0034](docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md) | One YAML configuration file for the orchestrator (`ORCH_CONFIG_FILE`, `version: 1`, unknown keys refused, every error at once, exit 78), secrets only as `{env}` or `{file}` references, a committed JSON Schema; the variables of today win over it for one release, then go; the web reads only its public `ui` subset from `GET /api/config`; amends 0009. Keys: [`docs/api/config.md`](docs/api/config.md). *Accepted 2026-10-02 (owner's request); built in S9, `GET /api/config` with S18* |
| [0035](docs/decisions/0035-utility-model-tasks.md) | Utility model tasks: named model endpoints (`ChatRequest.endpoint`), a closed `TaskKind` (title, description), each with its endpoint, model, prompt and language rule; the core always adds the fence, the data clause and the language line; a thread's description (`thread_described`), asked at most once per job, a person's edit final; extends 0005. *Accepted 2026-10-02 (owner's request); built (S18, S19: the core, `orch-model-openai`, `GET /api/config`, the web)* |
| [0036](docs/decisions/0036-sending-while-an-agent-works.md) | Sending while an agent works: **Send** is steer (the running task reads it at its next step when the agent's card lists `steer/v1`, else it is delivered after the turn), **Stop & send** cancels the running task and starts the next job with the text, naming the cancelled task in `referenceTaskIds`; `user_message.delivery`, `Job.after_stop` and its table, no verification and no rework for a job a person abandoned, a `steer` outbox kind, open question 33 decided (`Redeliver { sent: true }`); amends 0020, 0012 and 0018. Contract: [`docs/api/steer-v1.md`](docs/api/steer-v1.md). *Accepted 2026-10-02 (owner's request, details on delegation); built: the core and application (PR-11), the AG-UI member, the steer row and the web, with `dev/steer-e2e.sh`; the adam-rs side is adam-rs #75* |
| [0037](docs/decisions/0037-the-mvp-is-complete-against-its-build-order.md) | The MVP is complete against its build order: every row of the vision's requirement tables is built and asserted on mocks, or listed as post-MVP with a reason; complete is not accepted, and live verification is on the post-MVP list. *Proposed 2026-10-03 (the planner; the owner has not agreed)* |
| [0038](docs/decisions/0038-one-agent-definition-linked-into-each-tool.md) | Development agents: one canonical file per agent in `.agents/agents/`, a portable frontmatter (no `tools`, `model` or `color`), relative symlinks for Claude Code and OpenCode, generated adapters for Kiro, Codex and Gemini that point at the file and never copy it, the five tool directories generated-only by `tools/agents/sync.mjs`, checked in CI. *Accepted 2026-10-03 (owner's request)* |
| [0039](docs/decisions/0039-nobody-reads-another-persons-thread.md) | Nobody reads another person's thread: no role, `admin` included, reads or acts on a thread that is not the person's (`Scope` has one value, `own`; a role's `scope: any` is refused at startup, exit 78, naming the key; `GET /api/threads?owner=` is a 400 and `list_all_threads` is gone; the web's Mine / All and "Read only: someone's thread" go); `admin` stays as an operational, content-free permission; break-glass is database access by the operator, outside the application; reverses owner decision 4 of 0033. *Accepted 2026-10-03 (owner's decision: "Admins shouldn't read every thread, it's dangerous and not GDPR compliant"); built in S-A* |
| [0040](docs/decisions/0040-thread-sharing-by-revocable-link.md) | Thread sharing: a thread is `private`, `internal` (any signed-in person with the link) or `public` (anybody with the link), read-only and revocable; the deployment's `sharing.mode` (`disabled` by default) caps it at read time; the link is a token of its own (a random nonce and a MAC under a deployment secret, never the thread id), `thread_shared` and `thread_unshared` in the log, migration `0015`, the permission `thread.share`, a reader projection that never shows the owner's e-mail and hides step input and output and files from the public, a public route outside sign-in and rate limited before `public` is allowed. *Accepted 2026-10-03 (owner's concept and defaults); backend built (S-B2, 2026-10-03): the log, migration `0015`, the routes, the reader projection and the limiter, proven on mocks; the web, the edge and the end-to-end script are not built* |
| [0041](docs/decisions/0041-deployed-with-helm-on-kubernetes-secrets-by-externalsecret.md) | Deployed with Helm on Kubernetes; secrets by ExternalSecret: the chart `deploy/chart` in this repository (Argo CD in home-os tracks its `HEAD`), v0 = the orchestrator as one process, web, oauth2-proxy and a Caddy edge behind a Traefik Ingress, CNPG, the chat agent; `server.environment: production`, `auth.mode: jwt` and `defaultRole: null` are not values; every secret one property of the AWS secret `prod/another-agentic/env` read through `ssegning-aws`, one Secret per consumer; first-party images by commit tag bumped by CI, third-party by tag and digest; the web image is built with the sign-in path; Keycloak made by hand and kept as exports without secrets; extends 0033 and 0034. *Accepted 2026-10-03 (the owner's answers to the plan); built: the chart, its CI and the exports; not run on a cluster* |
| [0042](docs/decisions/0042-the-thread-list-is-the-owners.md) | The thread list is the owner's: pin, archive, order and nesting are columns of the thread row with no event in the log (amends invariant 3 and 0001), a fractional rank computed by the server from an anchor, a tree one level deep (a fork is nested under its parent's root; "Eject from parent" ungroups it), Pinned / Chats / Archived in manual order in place of the recency groups, archiving does not stop a share; a fork is made by its first message (a client-only draft at `fork:<parent>`, then `vymalo.fork` on the AG-UI run makes the fork and its message in one store transaction, `router.replace` to the new id; REST `forkThread` takes `after` with `text`); `PATCH /api/threads/{id}/rail`, `listThreads` `order` and `archived`; extends 0029, 0001 and 0040. *Proposed 2026-10-03 (the owner's words about a lazy fork, a row menu, a tree and drag and drop; the details are the planner's); not built* |
| [0043](docs/decisions/0043-deleting-a-thread-erases-it.md) | Deleting a thread erases it: `DELETE /api/threads/{id}` hard-deletes the thread and its edit branches (forks are kept and take its place in the list), in one transaction that also writes a `thread_purges` row, then `ArtifactStore::delete_prefix` runs inline or in a sweep; refused with `409` while a job runs ("Stop and delete" in the web); its share is a 404 at once; late input to a missing thread is dropped; the permission `thread.delete`; what is out of reach is stated (the agent's own store, the model provider, backups, an `agent-local` journal); a fork copies its files first; closes the deletion half of question 46, amends 0040 and 0032. *Proposed 2026-10-03 (the planner's; the owner asked for Delete in the row menu); not built* |
| [0044](docs/decisions/0044-a-models-reasoning-is-shown-beside-the-answer-and-logged-once.md) | A model's reasoning is shown beside the answer and logged once: `text-stream/v1` chunks marked `kind: "reasoning"` are relayed live on a second lane (`LiveKind::Reasoning`, wakeup payload `y` + `k: "r"` so an older replica drops it) and collected by the A2A mapper into one `AgentUpdate::Reasoning`, which the core logs as one `agent_reasoning` event (32 KiB, `truncated` marked; migration 0018); AG-UI says it as the five `REASONING_*` events before the turn's words, live first and continued by the log (`vymalo.live`, `~final` for a given-up one); never the agent's words, a title's, a fork's or an asked agent's answer; a public reader sees it only with `stepIo`; the web draws a closed "Thinking" block that streams while open; replaces the "Rejected: `REASONING_*`" bullet of `agui.md`; roll the orchestrator out before the agents (adam-rs ADR 0020). *Accepted 2026-10-05 (the owner's delegation); built and proven on mocks, no real model or gateway tried* |
| [0045](docs/decisions/0045-admin-dashboard-in-the-web-and-agent-access-from-the-registry.md) | The admin dashboard is an area of the web; permissions are Keycloak client roles bundled by composite roles; who may use a platform agent comes from its registry entry: `/admin` in the web, shown when `PLATFORM_API_URL` is set and the Platform API answers, drawn for people who hold the dashboard's permissions (`GET /v1/me`; proposed names `platform:agents.read`, `platform:agents.write`, `platform:models.write`, `platform:toolproviders.write`, `platform:secrets.pick`), its one route handler forwarding the edge's bearer to the Platform API, which checks permissions and never a role name; an agent's `audience` in its registry item lists its use permission, a client role `agent.use:<agent-name>` that composites grant, and narrows `auth.roles` for platform agents in every agent check (the orchestrator's check stays audience ∩ the person's roles; an item without one is for administrators only, static agents unchanged); the dashboard takes over `coder` and `chat` at the platform's cutover (their configuration then lives only in the cluster); one coder per GitHub owner with one GitHub App each (`coder` renamed `coder-vymalo`, `coder-me` for `stephane-segning`, noted in 0041); specified with the platform's architecture §60a (AD-025 to AD-033); extends 0022 and 0033, would amend the web's "no server-side fetches" when built; 0033 and 0039 not amended. *Accepted 2026-10-05 (the owner's answers to the thirteen questions of the platform's §93: two changed a recommendation, permissions as composite roles and the takeover of `coder` and `chat`; the permission names are proposed, the Keycloak composite expansion unverified); not built*  |
| [0049](docs/decisions/0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md) | The coder is shown as Adam, an agent may have aliases, and a step may carry the OpenCode icon: the agent's id is `adam` and its name `Adam` (the image, the chart, the Service and the environment variables of the coder keep their names); an entry of `AGENTS_FILE` may list `aliases`, other names that a request, a mention, a path, a link or an older thread resolves to the agent, while the API lists only the canonical id (with `Agent.aliases`) and a new thread, a fork and every mention written are created under it; a thread keeps the id it was created with (the log is not rewritten) and is read through the alias by the registry, the roles, the tool servers, the run's URL and `ask_agent`; a collision of an alias with an id or another alias is a startup error; `opencode` joins the `steps/v1` icon vocabulary, drawn as a neutral terminal glyph (OpenCode's logo terms are unverified), once adam-rs sends it; adam-rs's ADR 0021 is the agent's side (not merged). *Accepted 2026-10-06 (the owner: "It's coder, but just because it can code."); built and proven on mocks and unit tests only* |
| [0050](docs/decisions/0050-the-chat-has-sub-agents.md) | The chat agent has sub-agents, a researcher, a writer and a planner, as files of its folder (`subagents/`): each is a tool of the chat and a child run with its own prompt and tools; a sub-agent inherits nothing, not even the conversation's tools (read in adam-rs `6478fbc`, not run), so the researcher has its own search (`mcp.json`: the mock in dev, the search pod in the chart, none and an honest "I could not search" without one); the chat's instructions say when to use each and to confirm a plan with the person; proven by a `[mock:plan]` script of the scripted model. *Accepted 2026-10-06 (the owner: "even a simple chat shall some", and his choice of the three); built and proven on mocks only* |

## Local development

`compose.yaml` runs everything except the agents' real work: Postgres, two
[WireMock](https://wiremock.org/) stand-ins for an A2A 1.0 coding agent, and, with the `app`
profile, the real orchestrator and chat UI behind one origin, plus the default agent, adam-coder (shown as Adam, ADR 0049)
([ADR 0014](docs/decisions/0014-adam-coder-default-agent-over-a2a.md)), on scripted mocks of its
model, GitHub and git remote, and a CI stand-in, and two more agents that are only a folder each, a chat and a researcher
([Several agents](dev/README.md#several-agents)). Docker with Compose v2 is all it needs; the mocks need no agent host,
model or GitHub token. **Start with [Test it locally](dev/README.md#test-it-locally)** (prerequisites, URLs, what the chat shows, MCP,
going live, troubleshooting); the rest of [`dev/README.md`](dev/README.md) is the reference and the scenarios.

```sh
docker compose up -d --wait                          # postgres + mocks: nothing is built, seconds
docker compose --profile app up --build              # the whole system (first build takes minutes, the coder image is 2.9 GB); add -d --wait to return when healthy
open http://127.0.0.1:8080                           # the chat UI; the coder is preselected, "Mock coder" is one click away
dev/e2e-all.sh                                       # every scenario against the running stack, then a summary (curl, jq, git, openssl)
dev/greeting-e2e.sh                                  # or one of them: "hi" gets a greeting that says the coder's name, not a request for a task
dev/agents-e2e.sh                                    # or one of them: the coder, a chat and a researcher each answer in their role (the researcher cites a mock search result)
dev/coder-e2e.sh                                     # or one of them: a chat message becomes a pull request, gated on the coder's checks and CI
dev/ci-e2e.sh                                        # a gated mock agent: a signed CI report sends it back, then ends the job
dev/try-thread.sh "add a health endpoint"            # or drive a mock thread from the terminal (curl, jq)
dev/mcp-e2e.sh                                       # or start a job as an MCP client would, with a bearer token (curl, jq)
docker compose --profile app down -v                 # stop and forget the database
```

The `split` profile adds two workers, so the orchestrator can run as a control plane beside them
([`dev/README.md`](dev/README.md#the-split-profile-a-control-plane-and-two-workers)):

```sh
ORCHESTRATOR_ROLE=control-plane docker compose --profile app --profile split up -d --build --wait
dev/split-e2e.sh                                     # kills the worker that holds a task; the other finishes it
```

| Profile | Services | Ports on 127.0.0.1 |
|---|---|---|
| default | `postgres`, `mock-agent`, `mock-agent-releases`, `mock-verifier` | 5432, 8081, 8082, 8083 |
| `app` | + `orchestrator`, `web`, `edge` | 8080 (`/api/*` to the orchestrator, the rest to the UI) |
| `split` | + `orchestrator-worker-1`, `orchestrator-worker-2` (dispatcher only; beside `app`, with `ORCHESTRATOR_ROLE=control-plane`) | none published |
| `app` | + `coder`, `coder-postgres`, `mock-openai`, `mock-github`, `git-server` (the default agent and its mocks), `mock-ci` (a CI stand-in: the coder is gated on its own checks and on CI and ends `done` when `mock-ci` has reported the pushed commit, [`dev/README.md`](dev/README.md#ci-the-gate-by-webhook)), `mock-mcp-search` (a mock web-search MCP server with canned results, [`dev/README.md`](dev/README.md#mock-web-search-mcp)), `chat`, `researcher`, `agents-postgres` and `mock-model` (two more agents, each only a folder under [`dev/agents/`](dev/agents/) served by `adam-agent` from the coder's image, on a scripted model; the researcher searches through the mock web search; [Several agents](dev/README.md#several-agents), and [how to add a fourth](dev/README.md#add-a-fourth-agent-by-writing-a-folder)). The coder reads its agent folder (name, card, instructions) from [`dev/coder/agent/`](dev/coder/agent/instructions.md), mounted at `/etc/adam/agent`: edit it and `docker compose --profile app up -d coder`, no rebuild ([Change what the coder says](dev/README.md#change-what-the-coder-says)) | 8090 (`coder`), 8091 (`mock-openai`), 8092 (`mock-github`), 8093 (`git-server`), 8094 (`mock-model`), 8096 (`mock-mcp-search`), 8097 (`chat`), 8098 (`researcher`); `coder-postgres` and `agents-postgres` are not published |
| `smee` | + `smee`, `smee-proxy` (opt-in: forwards GitHub webhooks from smee.io, a third party that sees them; needs `SMEE_URL`) | none published |
| `local-agent` | `orchestrator-local`, `local-postgres` (opt-in: the orchestrator built with `agent-local`, hosting an `echo` agent) | 8095 |

To point the coder at a real model and GitHub, copy [`.env.example`](.env.example) to `.env` and add the override:
`docker compose -f compose.yaml -f compose.live.yaml --profile app up --build` (Compose v2.24.4 or newer;
[`dev/README.md`](dev/README.md#going-live)).

Locally the sign-in is real and costs nothing to use: the `edge` proxy asks a real oauth2-proxy, which signs a browser in at `mock-oidc`, a mock issuer
that approves it as `dev@example.com` (a user; `admin@example.com` is an administrator), and the orchestrator validates the token as an OAuth2 resource server;
the scripts get a token from the mock. It is for a laptop, never for production
([`dev/README.md`](dev/README.md#sign-in-a-mock-issuer-and-oauth2-proxy)).

To run the code you are changing against the mocks (compose supplies only the infrastructure):

| Variable | Value | For |
|---|---|---|
| `DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/orch` | the orchestrator |
| `AGENTS_FILE` | `dev/agents.local.yaml` (agents on `127.0.0.1:8081`/`8082`/`8083`) | the orchestrator |
| `MOCK_AGENT_TOKEN` | `dev-mock-token` (any non-empty value; the mocks only require a bearer) | the orchestrator, named by `tokenEnv` |
| `AUTH_DEV_USER` | `dev@example.com` | the orchestrator without the edge proxy |
| `ORCH_TEST_DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/orch_test` | `cargo test --workspace` |
| `ORCH_TEST_MOCK_AGENT_URL`, `ORCH_TEST_MOCK_AGENT_RELEASES_URL`, `ORCH_TEST_MOCK_VERIFIER_URL` | `http://127.0.0.1:8081`, `http://127.0.0.1:8082`, `http://127.0.0.1:8083` | `cargo test -p orch-e2e --test wiremock_agent` |

The mock agent picks its script from a word in your message:

| Say | The thread |
|---|---|
| anything else | works, opens a "pull request" artifact, ends `done` |
| `ask` | asks a question and ends `blocked`; answer it in the same thread |
| `fail` / `reject` / `error` | ends `failed` (agent failure / agent rejection / JSON-RPC error) |
| `slow` | like the default, over 8 seconds |

`mock-agent-releases` declares the release-channels extension, so only it has a **Release** group in the
agent menu of a new chat: channels `production`, `staging`, `latest` and three revisions. Ports can be moved with
`POSTGRES_PORT`, `MOCK_AGENT_PORT`, `MOCK_AGENT_RELEASES_PORT`, `MOCK_VERIFIER_PORT`, `EDGE_PORT`, `CODER_PORT`,
`MOCK_OPENAI_PORT`, `MOCK_GITHUB_PORT`, `GIT_SERVER_PORT`, `MOCK_MODEL_PORT`, `MOCK_MCP_SEARCH_PORT`, `CHAT_PORT` and `RESEARCHER_PORT`. CI keeps the mocks
honest: [`compose.yml`](.github/workflows/compose.yml) starts them, runs
[`dev/check-mocks.sh`](dev/check-mocks.sh) (and [`dev/check-agent-mocks.sh`](dev/check-agent-mocks.sh) for the mock web search and the agents' scripted models) and the real orchestrator client against them, and
[`coder-e2e.yml`](.github/workflows/coder-e2e.yml) runs the whole `app` profile, coder included, and
checks that the mocks and the agent folder vendored under `dev/coder` still equal upstream.

The coder, the chat and the researcher are not reachable from an orchestrator running on the host (their cards advertise
`http://coder:8080/` and so on), so `dev/agents.local.yaml` leaves them out.

## Related

- **another-agentic-platform** — the agent platform: versioned agent services,
  release channels, runtimes, harnesses. This system consumes it over A2A like
  any other host, with an optional release picker.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Vendored agent skills keep their own
licenses; see [third-party-notices.md](third-party-notices.md).
