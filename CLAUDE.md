# Agent guide — another-agentic-system

`AGENTS.md` is a symlink to this file. Edit `CLAUDE.md` only.

## What this is

A protocol-agnostic **orchestration layer** for multi-agent work: a chat
surface (Next.js + assistant-ui) and a stateless Rust orchestrator over one
Postgres event log. It drives any A2A agent, uses tools over MCP, reacts to
webhooks, and can be driven the same way. **Status: MVP steps 1–2 are built**
(`orchestrator/`, `web/`), but that is plumbing: the owner judged the MVP not
ready. What the system is meant to be is `docs/vision.md`; the re-planned build
order is `docs/mvp.md`.

## Layout

| Path | What |
|---|---|
| `docs/vision.md` | What the system is meant to be (the owner's requirements), what exists and what is missing |
| `docs/architecture.md` | Components, agent hosts, job flow and lifecycle |
| `docs/orchestrator.md` | Ports & adapters, event/command model, inbox/outbox, core types, data model, crate layout |
| `docs/decisions/NNNN-*.md` | ADRs |
| `docs/mvp.md`, `docs/open-questions.md`, `docs/lessons-from-agent-canvas.md` | Build order, open/closed questions, lessons as requirements |
| `tools/docs-check/` | Diagram, link and image checker (also run in CI) |
| `tools/agui-conformance/` | Reads the AG-UI goldens through the reference client, `@ag-ui/client` 1.0.0 (also run in CI) |
| `compose.yaml`, `compose.live.yaml`, `.env.example`, `dev/` | Local stack; start at `dev/README.md` "Test it locally". `compose.yaml`: Postgres, WireMock A2A mock agents, and the `app` profile (orchestrator, configured by `dev/orchestrator.yaml` (`ORCH_CONFIG_FILE`, ADR 0034; `dev/orchestrator.live.yaml` for `compose.live.yaml`; secrets are `{ env }` references to dummy variables of the compose file), web, edge proxy standing in for oauth2-proxy, adam-coder the default agent, pinned by tag and digest, beside mocks and its agent folder (`dev/coder/agent/`, mounted at `/etc/adam/agent`: instructions and card read at startup, so a change needs a restart, no build) vendored from adam-rs into `dev/coder/`, see `dev/coder/UPSTREAM`; git-server; `mock-ci`, a CI stand-in that reports pushed commits to the webhook; `mock-mcp-search`, a dependency-free mock web-search MCP server, `dev/mock-mcp-search/` (live, `compose.live.yaml` swaps it for `searxng` and `searxng-mcp`, `dev/searxng-mcp/`, a dependency-free MCP server with `web_search` over a self-hosted SearXNG, Brave or Tavily and a `fetch` that reads a public page as text, SSRF-guarded: `dev/README.md` "Web search for real"); `mock-github-mcp`, the coder's read-only GitHub MCP server as a WireMock (vendored; `dev/compose.github-app.yaml` runs the coder as a GitHub App); the named volume `orchestrator-artifacts` (the files agents hand over, ADR 0032; every orchestrator role mounts it); `mock-registry`, a WireMock stand-in for the platform's agent registry (`agent-registry/v1`, the orchestrator reads it live: `AGENT_REGISTRY_URL`), `dev/wiremock/registry/`; and two agents that are only a folder each, `chat` and `researcher` (`dev/agents/<id>/agent/`, served by `adam-agent` from the coder's image, the researcher's `mcp.json` naming the mock web search; their instructions say replies render as Markdown and what the person sees, and `max_output_tokens` is 8192) on a scripted `mock-model` (`dev/wiremock/model/`, whose scripts have SSE twins because the agents stream, and which also plays `mock-title`, the model the orchestrator asks for thread titles) and one `agents-postgres`: adding a fourth is a folder and a dozen lines, `dev/README.md` "Add a fourth agent by writing a folder"); opt-in profiles `split` (two workers beside a control plane), `smee` (smee-client behind a Caddy that passes only `/webhooks/github`; smee.io is a third party) and `local-agent` (the orchestrator built with `agent-local`). `compose.live.yaml` is an override (real model and GitHub from `.env`, `dev/agents.live.yaml`; Compose v2.24.4+). Scenario scripts, one per scenario, each asserting the chain: `greeting-e2e.sh` ("hi" gets the coder's greeting), `agents-e2e.sh` (three agents, each answers in its role, the researcher's search a step with its input and output), `choices-e2e.sh` (the coder asks with a form drawn from the web's UI catalog, one action answers it), `cards-e2e.sh` (the researcher answers with one surface of cards and a graph), `title-e2e.sh` (a thread is titled by the orchestrator's own model after the agent's first reply; a person's rename is final), `description-e2e.sh` (a finished job gets the thread a description from a model of its own at an endpoint of its own, with the configured guidance; a person's description is final and a fork has it), `fork-e2e.sh` (a finished thread is forked through the API; the fork's first message reaches the agent with the conversation it continues in front of it, read from the mock agent's request journal), `registry-e2e.sh` (the platform's registry: its agent is listed after the static ones with the releases of its own card, an agent added shows up with no restart, a registry that is down leaves the static agents and the UI says so), `agent-folder-e2e.sh` (the coder restarted on an edited copy of its folder), `coder-e2e.sh` (chat to pull request, the coder's work as a tree of steps with each tool step's input and output, and its answer shown live, GitHub read over MCP, the credential a token or, with `GITHUB_AUTH=app`, a GitHub App), `workspace-e2e.sh` (the coder needs no repository: a repository it creates, or a second one it adds, only after the person answers a form with yes), `verify-e2e.sh`, `verifier-e2e.sh`, `ci-e2e.sh`, `mcp-e2e.sh`, `split-e2e.sh`, plus `e2e-all.sh` (all but split), `check-mocks.sh` and `check-agent-mocks.sh`, and `export-thread.sh` (saves a thread as one JSON file to send a developer, the script form of the web's Export JSON); CI runs them in `coder-e2e.yml`; `dev/README.md` documents the scenarios |
| `.agents/skills/` | Repo skills; `.claude/skills/*` are symlinks to them |

## Invariants — check every change against these

1. **Protocols only (ADR 0007).** No dependency on an agent host, gateway
   product or host SDK. An agent is an A2A agent-card URL; models are
   OpenAI-compatible endpoints, named in the configuration (ADR 0005, ADR 0035). *(Amended by ADR 0015: `adam-host` is a git
   dependency, and a worker may host adam agents in-process behind the off-by-default
   feature `agent-local`; remote agents stay plain A2A.)*
2. **Host conveniences are optional extensions (ADR 0008).** Capability-detected
   from a standard protocol's extension mechanism, read live, never cached,
   fail closed, removable without breaking plain A2A.
3. **Stateless processes, one event log (ADR 0001).** Only the job ledger and
   event log (the chat) persist, in Postgres. *(Amended by ADR 0015: a local agent's journal
   counts as job ledger. Amended by ADR 0032: the files agents hand over are durable outside
   Postgres, in an artifact store behind a port, by content hash; the log keeps only the
   reference.)*
4. **Verification over consensus (ADR 0002)** and **git is the artifact (ADR 0003).**
5. **The core is pure (orchestrator.md).** `transition(&state, &event)` has no
   I/O; protocols are closed enums (ADR 0004).
6. **Swappable implementations (ADR 0009).** Every infrastructure boundary is a
   trait in the `ports` crate, with a conformance testkit; implementations are
   separate crates; binaries are only compositions. No implementation types in
   trait signatures. Swapping happens at build time (features + config, or your
   own composition root) — not via runtime plugins.
7. **Naming:** this is the *orchestration layer*. "Harness" means an agent's
   internal framework in another-agentic-platform — don't reuse it here.

## Skills

Skills live in `.agents/skills/` (symlinked into `.claude/skills/`). Most are
vendored from `addyosmani/agent-skills`, `actionbook/rust-skills` and
`leonardomso/rust-skills` and pinned in `skills-lock.json` — update them with
the skills CLI, never by hand-editing their files.

**Precedence when they disagree:** this file's *Invariants* → the repo's own
skill (`write-adr`) → vendored skills. For example, `documentation-and-adrs`
carries its own ADR template; ADRs here use `write-adr`'s format and numbering.

Start with `using-agent-skills` if unsure which applies.

| When you are… | Use |
|---|---|
| Recording, amending or superseding a decision | **`write-adr`** (repo skill); `documentation-and-adrs` only for the reasoning style |
| Turning a vague request into a design | `idea-refine`, `interview-me` (ask the owner one question at a time) |
| Writing a spec for a feature or MVP step | `spec-driven-development`, then `planning-and-task-breakdown` |
| Making a decision that is hard to reverse | `doubt-driven-development` (adversarial review before it stands) |
| Checking a claim about a library, protocol or product | `source-driven-development` — and mark it *verified* with date + source |
| Designing a port, trait, protocol adapter or public API | `api-and-interface-design`; Rust side: `m04-zero-cost`, `m05-type-driven` |
| Implementing anything | `incremental-implementation` + `test-driven-development` |
| Rust: first stop for any Rust question | `rust-router`, which dispatches to the `m01`…`m15` skills |
| Rust: borrow-checker, ownership, smart pointers, mutability errors | `m01-ownership`, `m02-resource`, `m03-mutability` |
| Rust: errors | `m06-error-handling`, `m13-domain-error` (house rule: `thiserror` in libraries, `anyhow` in binaries) |
| Rust: async, Postgres claims, dispatcher loops | `m07-concurrency`, `m12-lifecycle` |
| Rust: the event/command model | `m09-domain`, `m05-type-driven` |
| Rust: crates, workspace, features | `m11-ecosystem`, `rust-learner` (versions), `rust-deps-visualizer` |
| Rust: navigating or refactoring code | `rust-code-navigator`, `rust-symbol-analyzer`, `rust-trait-explorer`, `rust-call-graph`, `rust-refactor-helper` |
| Rust: rules catalogue / anti-patterns | `rust-skills`, `coding-guidelines`, `m15-anti-pattern`; `unsafe-checker` if `unsafe` ever appears |
| HTTP/SSE/A2A/MCP adapters (axum, reqwest) | `domain-web`, `security-and-hardening` (inbound auth is fail-closed) |
| Deploying (Kubernetes, containers, probes) | `domain-cloud-native`, `shipping-and-launch` |
| Chat surface (Next.js + assistant-ui) | `frontend-ui-engineering`, `browser-testing-with-devtools` |
| Logs, metrics, traces | `observability-and-instrumentation` |
| Throughput or latency of the orchestrator/dispatcher | `performance-optimization`, `m10-performance` |
| Something broke | `debugging-and-error-recovery` |
| Before opening or merging a PR | `code-review-and-quality`, `code-simplification`, `git-workflow-and-versioning` |
| CI workflows | `ci-cd-and-automation` |
| Replacing an implementation or retiring an API | `deprecation-and-migration` |
| Setting or raising the quality bar | `constraint-driven-development` |
| Editing this file or other agent context | `context-engineering` |

Not for direct use: `core-actionbook`, `core-agent-browser`, `core-dynamic-skills`,
`core-fix-skill-docs` (internal helpers invoked by other rust-skills workflows),
`meta-cognition-parallel` (experimental), `rust-skill-creator`, `rust-daily`,
`m14-mental-model` (learning aids). Off-domain here: `domain-cli`,
`domain-embedded`, `domain-fintech`, `domain-iot`, `domain-ml`.

## Writing docs

- **Decisions are ADRs.** New file `docs/decisions/NNNN-kebab-title.md` (next
  number), added to the README's Decisions table. Change a past decision by
  amending it with a dated status note, or superseding it with a new ADR —
  never silently rewriting it. Skill: `write-adr` (see *Skills*).
- **Mark facts** as *verified* (with date and source) or *unverified*.
- **Screenshots are the web's own.** `pnpm screens` (in `web/`) writes `web/e2e/__screens__/<device>-<scheme>-<state>.png`;
  docs link to those files (never copies), as a `<picture>` of the `-light-` and `-dark-` file, with alt text that says what the
  screen shows and a caption that says it comes from the web's mock server. Renaming a screen breaks the docs that embed it:
  `tools/docs-check` fails on it (it reads `![]()` and the `src` and `srcset` of HTML tags).
- **Processes are diagrams.** A Mermaid pair — `sequenceDiagram` for the
  interaction, `stateDiagram-v2` for the lifecycle — then prose.
- Open questions move between Open / Closed / Moved in `docs/open-questions.md`;
  don't delete them.

## Code (when it starts)

Every crate under `orchestrator/crates/` and `orchestrator/bin/` has a
`README.md` next to its `Cargo.toml` (and `readme = "README.md"` in the
manifest). Update it in the same PR as any change to the crate's public API,
environment variables or tests. `tools/docs-check` fails when a crate has no
README and checks its relative links; it cannot check accuracy, so review does.

Rust: `thiserror` in library crates, `anyhow` in binaries, no `f64` for time or
money, `jiff` for time, `sqlx` + `axum`. The `core` crate stays free of async
and I/O so the compiler enforces purity.

## Commands

```sh
npm --prefix tools/docs-check ci          # once per clone
node tools/docs-check/check-docs.mjs      # every diagram parses, every relative link and image resolves
npm --prefix tools/agui-conformance ci    # once per clone, for the next line
node tools/agui-conformance/check.mjs     # every AG-UI golden reads cleanly through the reference client
git config core.hooksPath .githooks       # once per clone: local Conventional Commits hook
```

## Commits

Conventional Commits, enforced by `tools/commit-lint.sh` (local hook and CI):
`<type>[(scope)][!]: <description>`, types
`feat fix docs style refactor perf test build ci chore revert`. The PR title
is validated too.

## Pull requests

- Work on a branch; open a PR against `main`. `gh` and `git push` need the
  interactive zsh profile for credentials: `zsh -i -c 'git push …'`.
- The body must follow `.github/PULL_REQUEST_TEMPLATE.md` (AI governance):
  Summary with a source-of-truth link, Intent, Scope, Verification with
  evidence, Risk, AI Usage Declaration, Reviewer Focus. The `AI Governance`
  check fails otherwise. Source: https://adorsys-gis.github.io/ai-governance/
- Checks: `AI Governance`, `Commit Lint`, `Docs`.

## Related repositories

- `vymalo/another-agentic-platform` — the agent platform (first-class, optional agent host).
- `vymalo/another-agentic-images` — toolchain images.
- `vymalo/another-adam-rs` — adam-coder, the default A2A coding agent; also the adam-rs library (see the pending ADR 0015).
