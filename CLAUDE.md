# Agent guide — another-agentic-system

`AGENTS.md` is a symlink to this file. Edit `CLAUDE.md` only.

## What this is

A protocol-agnostic **orchestration layer** for multi-agent work: a chat
surface (Next.js + assistant-ui) and a stateless Rust orchestrator over one
Postgres event log. It drives any A2A agent, uses tools over MCP, reacts to
webhooks, and can be driven the same way. **Status: design only** — there is
no code yet.

## Layout

| Path | What |
|---|---|
| `docs/architecture.md` | Components, agent hosts, job flow and lifecycle |
| `docs/orchestrator.md` | Ports & adapters, event/command model, inbox/outbox, core types, data model, crate layout |
| `docs/decisions/NNNN-*.md` | ADRs |
| `docs/mvp.md`, `docs/open-questions.md`, `docs/lessons-from-agent-canvas.md` | Build order, open/closed questions, lessons as requirements |
| `tools/docs-check/` | Diagram + link checker (also run in CI) |
| `.agents/skills/` | Repo skills; `.claude/skills/*` are symlinks to them |

## Invariants — check every change against these

1. **Protocols only (ADR 0007).** No dependency on an agent host, gateway
   product or host SDK. An agent is an A2A agent-card URL; models are one
   OpenAI-compatible endpoint (ADR 0005).
2. **Host conveniences are optional extensions (ADR 0008).** Capability-detected
   from a standard protocol's extension mechanism, read live, never cached,
   fail closed, removable without breaking plain A2A.
3. **Stateless processes, one event log (ADR 0001).** Only the job ledger and
   event log (the chat) persist, in Postgres.
4. **Verification over consensus (ADR 0002)** and **git is the artifact (ADR 0003).**
5. **The core is pure (orchestrator.md).** `transition(&state, &event)` has no
   I/O; protocols are closed enums (ADR 0004).
6. **Naming:** this is the *orchestration layer*. "Harness" means an agent's
   internal framework in another-agentic-platform — don't reuse it here.

## Writing docs

- **Decisions are ADRs.** New file `docs/decisions/NNNN-kebab-title.md` (next
  number), added to the README's Decisions table. Change a past decision by
  amending it with a dated status note, or superseding it with a new ADR —
  never silently rewriting it. Skill: `write-adr`.
- **Mark facts** as *verified* (with date and source) or *unverified*.
- **Processes are diagrams.** A Mermaid pair — `sequenceDiagram` for the
  interaction, `stateDiagram-v2` for the lifecycle — then prose.
- Open questions move between Open / Closed / Moved in `docs/open-questions.md`;
  don't delete them.

## Code (when it starts)

Rust: `thiserror` in library crates, `anyhow` in binaries, no `f64` for time or
money, `jiff` for time, `sqlx` + `axum`. The `core` crate stays free of async
and I/O so the compiler enforces purity.

## Commands

```sh
npm --prefix tools/docs-check ci          # once per clone
node tools/docs-check/check-docs.mjs      # every diagram parses, every relative link resolves
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
