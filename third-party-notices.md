# Third-party notices

This repository vendors agent skills from third-party projects under
`.agents/skills/` (pinned in `skills-lock.json`). Their licences:

| Source | Licence | Notice | Skills |
|---|---|---|---|
| [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) | MIT | [`third-party-licenses/addyosmani-agent-skills-MIT.txt`](third-party-licenses/addyosmani-agent-skills-MIT.txt) — Copyright (c) 2025 Addy Osmani | `api-and-interface-design`, `browser-testing-with-devtools`, `ci-cd-and-automation`, `code-review-and-quality`, `code-simplification`, `constraint-driven-development`, `context-engineering`, `debugging-and-error-recovery`, `deprecation-and-migration`, `documentation-and-adrs`, `doubt-driven-development`, `frontend-ui-engineering`, `git-workflow-and-versioning`, `idea-refine`, `incremental-implementation`, `interview-me`, `observability-and-instrumentation`, `performance-optimization`, `planning-and-task-breakdown`, `security-and-hardening`, `shipping-and-launch`, `source-driven-development`, `spec-driven-development`, `test-driven-development`, `using-agent-skills` |
| [actionbook/rust-skills](https://github.com/actionbook/rust-skills) | MIT (declared in the upstream README; the upstream repository ships no LICENSE file — checked 2026-09-28) | [upstream README, *License*](https://github.com/actionbook/rust-skills#license) | `coding-guidelines`, `core-actionbook`, `core-agent-browser`, `core-dynamic-skills`, `core-fix-skill-docs`, `domain-cli`, `domain-cloud-native`, `domain-embedded`, `domain-fintech`, `domain-iot`, `domain-ml`, `domain-web`, `m01-ownership`, `m02-resource`, `m03-mutability`, `m04-zero-cost`, `m05-type-driven`, `m06-error-handling`, `m07-concurrency`, `m09-domain`, `m10-performance`, `m11-ecosystem`, `m12-lifecycle`, `m13-domain-error`, `m14-mental-model`, `m15-anti-pattern`, `meta-cognition-parallel`, `rust-call-graph`, `rust-code-navigator`, `rust-daily`, `rust-deps-visualizer`, `rust-learner`, `rust-refactor-helper`, `rust-router`, `rust-skill-creator`, `rust-symbol-analyzer`, `rust-trait-explorer`, `unsafe-checker` |
| [docker/skills](https://github.com/docker/skills) | Apache-2.0 (upstream README, *License*, and its `LICENSE` file — checked 2026-10-03) | [`LICENSE`](https://github.com/docker/skills/blob/main/LICENSE) upstream | `docker-agent-config`, `docker-agent-deploy`, `docker-agent-run`, `docker-build-strategies`, `docker-compose-patterns`, `docker-destructive-guardrails`, `docker-project-foundations`, `docker-sandboxes-env`, `docker-sandboxes-kits`, `docker-sandboxes-lifecycle`, `docker-sandboxes-network-credentials` |
| [leonardomso/rust-skills](https://github.com/leonardomso/rust-skills) | MIT | [`rust-skills/LICENSE`](.agents/skills/rust-skills/LICENSE) — Copyright (c) 2025 Leonardo Maldonado | `rust-skills` |
| [vymalo/another-adam-rs](https://github.com/vymalo/another-adam-rs) | MIT OR Apache-2.0 (the `license` of its workspace `Cargo.toml`, checked 2026-10-03) | [`LICENSE-APACHE`](https://github.com/vymalo/another-adam-rs/blob/main/LICENSE-APACHE) and [`LICENSE-MIT`](https://github.com/vymalo/another-adam-rs/blob/main/LICENSE-MIT) upstream; the same owner and the same licences as this repository | `adam-a2a-extensions`, `adam-agent-folder`, `adam-coder-deploy`, `adam-embed`, `adam-store-adapter`, `adam-upgrade` |

## Logos bundled in the web

The web draws two third-party marks, small and in the colour of the text around them, as SVG components under
`web/src/components/brand/`. Each is a mark of its owner, used to say which product a step belongs to, never as this
project's own. *Verified 2026-10-11*, from the sources below
([ADR 0049](docs/decisions/0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md), amended).

| Mark | File | Source | Terms | Notice |
|---|---|---|---|---|
| GitHub mark (the Invertocat) | `web/src/components/brand/github-mark.tsx` | the path of [`icons/mark-github-16.svg`](https://github.com/primer/octicons/blob/main/icons/mark-github-16.svg) of primer/octicons, unchanged | Octicons' code is MIT, (c) GitHub Inc. ([`LICENSE`](https://github.com/primer/octicons/blob/main/LICENSE)); its README, *License*, leaves the logos to the [GitHub logo guidelines](https://brand.github.com/foundations/logo): "Use a permitted GitHub logo to inform others that your project integrates with GitHub", unmodified, in white, black or grey, and "less prominently than your own" name or logo, never suggesting that GitHub made or endorses the product. The same page limits the standalone Invertocat to "GitHub-owned environments or where the brand is already clearly established": the web draws it only beside a call of GitHub's MCP server, named GitHub in its tooltip and to a screen reader, never as an icon of this project (a judgment of ours, flagged for review in the ADR) | [`third-party-licenses/primer-octicons-MIT.txt`](third-party-licenses/primer-octicons-MIT.txt) for the Octicons code; the mark itself is GitHub's trademark |
| OpenCode mark | `web/src/components/brand/opencode-mark.tsx` | the square logo of [`packages/console/app/src/asset/brand/`](https://github.com/anomalyco/opencode/tree/dev/packages/console/app/src/asset/brand) in anomalyco/opencode (formerly sst/opencode), the assets its [brand page](https://opencode.ai/brand) offers for download, redrawn in a 16 x 20 box with the same two shapes | MIT, (c) 2025 opencode ([`LICENSE`](https://github.com/anomalyco/opencode/blob/dev/LICENSE) of the repository, no other licence file on the path of the assets), and the brand page ("Resources and assets to help you work with the OpenCode brand") states no further terms. No trademark policy was found: the mark is used to name OpenCode where the step hands work to it | [`third-party-licenses/opencode-MIT.txt`](third-party-licenses/opencode-MIT.txt) |

Not bundled: the **Model Context Protocol logo**. The repository that holds it
([modelcontextprotocol/modelcontextprotocol](https://github.com/modelcontextprotocol/modelcontextprotocol)) states Apache-2.0 for new
code and specification contributions, CC-BY-4.0 for documentation and MIT for contributions not yet relicensed, none of which
names the logo, and the Linux Foundation's [trademark policy](https://www.linuxfoundation.org/legal/trademark-usage) asks for
written permission to use its logos in marketing material. The use of the logo in a product is *unverified*, so the web
draws a glyph of a tool.

Everything else in this repository is the project's own work.
