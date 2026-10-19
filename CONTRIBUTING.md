# Contributing to another-agentic-system

Thank you for helping. This repository is the **orchestrator** (Rust, one Postgres event log) and the **chat surface**
(Next.js) of a family of four repositories. The product roadmap, with every open piece of work as a GitHub issue,
is in [`ROADMAP.md`](https://github.com/vymalo/another-agentic-system/blob/main/ROADMAP.md). Start there.

## Find something to work on

1. Pick an issue from the [roadmap](https://github.com/vymalo/another-agentic-system/blob/main/ROADMAP.md): a
   `good first issue`, a `help wanted` ticket, or any ticket whose "Who and when" says anyone.
2. **Claim it** by commenting on the issue. If you stop, say so.
3. On a large or security ticket, or one marked "design first", **agree the approach in the issue** before you write
   code.

Read the seven invariants in [`CLAUDE.md`](CLAUDE.md) before you change code. In short: protocols only, no agent host or
SDK; optional extensions, detected live and failing closed; stateless processes over one event log; a pure core; and
every infrastructure boundary is a trait in the `ports` crate with a testkit. `docs/architecture.md` and
`docs/orchestrator.md` explain the rest.

## Set up, build and test

You need Rust (the orchestrator pins its toolchain in `orchestrator/rust-toolchain.toml`), Node 24 with pnpm for
the web, and Docker with Compose v2 for the local stack. A Postgres 16 is optional: database tests skip without one.

```sh
git config core.hooksPath .githooks       # once per clone: local Conventional Commits hook
npm --prefix tools/docs-check ci          # once per clone
node tools/docs-check/check-docs.mjs      # every diagram parses, every relative link and image resolves
npm --prefix tools/agui-conformance ci    # once per clone, for the next line
node tools/agui-conformance/check.mjs     # every AG-UI golden reads cleanly through the reference client
node tools/agents/sync.mjs --check        # after editing .agents/agents: run it without --check to write the links
sh deploy/chart/tests/render-check.sh     # needs helm: the chart's guarantees on its render and its refusals
```

```sh
cd orchestrator
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                    # no database: the Postgres variants skip
ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test cargo test --workspace   # all of it
```

```sh
cd web
pnpm install --frozen-lockfile
pnpm check && pnpm typecheck && pnpm test && pnpm build
pnpm exec playwright install --with-deps chromium    # once
pnpm test:e2e                             # the specs, on the web's own mock server
```

To run the whole system on mocks (no account, no key, no network after the build), follow
[Test it locally](dev/README.md#test-it-locally):

```sh
docker compose --profile app up --build
```

The `dev/*-e2e.sh` scenario scripts are described in [`dev/README.md`](dev/README.md). CI is the reference for what must
pass: [`.github/workflows`](.github/workflows).

## Conventions

- **Conventional Commits**, enforced by `tools/commit-lint.sh` in the local hook and in CI. Shape:
  `<type>[(scope)][!]: <description>`. Types: `feat fix docs style refactor perf test build ci chore revert`. The pull
  request title is checked too, and so is every commit.
- **Decisions are ADRs.** Add `docs/decisions/NNNN-kebab-title.md` (the next number) and a row in the README's
  Decisions table. Change a past decision with a dated status note or a new ADR, never by rewriting it. The skill
  `write-adr` in `.agents/skills/` has the format.
- **Mark facts** about third parties and about what runs as *verified* (date and source) or *unverified*.
  Say what is proven on mocks and what is not.
- **Docs.** A process is a Mermaid pair: a `sequenceDiagram` for the interaction and a `stateDiagram-v2` for the
  lifecycle. Open questions move between Open, Closed and Moved in `docs/open-questions.md`; they are never deleted.
  Screenshots are the web's own (`pnpm screens`), linked and never copied.
- **Crate READMEs.** Every crate under `orchestrator/crates/` and `orchestrator/bin/` has a `README.md` next to its
  `Cargo.toml`. Update it in the same pull request as any change to the crate's public API, environment variables or
  tests. `tools/docs-check` fails on a missing one.
- **Rust.** `thiserror` in libraries, `anyhow` in binaries, no `f64` for time or money, `jiff` for time, `sqlx` and
  `axum`. The `core` crate stays free of async and I/O. Never edit a migration that has been applied: CI refuses it.
- **Secrets** are references in the configuration (ADR 0034). Never put one in a file, a log or an issue.

## Open a pull request

1. Work on a branch and open a pull request against `main`. Keep it to one concern.
2. Give it a Conventional Commit title.
3. Fill in [`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md), the AI-governance template. Its
   sections are Summary (with a link to the source of truth, such as the issue), Intent, Scope, Verification (with
   evidence: the commands you ran and their results), Screenshots or evidence, Risk, AI Usage Declaration and Reviewer
   Focus. The `AI Governance` check fails if the body is missing them.
4. These checks must pass: `AI Governance`, `Commit Lint` and `Docs`, and the workflows for the paths you changed
   (`orchestrator`, `web`, `deploy`, ...).
5. A maintainer merges the pull request when CI is green.

## AI-assisted contributions

They are welcome, and you declare them in the *AI Usage Declaration* of the pull request: what the AI did, and that you
read, understand and accept responsibility for every change, generated tests included. AI output is not evidence:
run the checks yourself. The rules are in the [AI governance](https://adorsys-gis.github.io/ai-governance/) guide this
repository follows. If you use a coding agent, point it at [`CLAUDE.md`](CLAUDE.md) (`AGENTS.md` is a link to it).

## Security

Do not report a vulnerability in a public issue. See [`SECURITY.md`](SECURITY.md).

## Licence

This repository is licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option. A contribution you submit is licensed the same way.
