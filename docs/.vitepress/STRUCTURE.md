# The shared structure of the documentation sites

another-agentic-system and another-agentic-platform publish one VitePress site each, built from the repository's own
`docs/` with `@vaam-apps/vitepress-md3`, so a pull request updates its docs in the same change. Both use the same
sections, in this order, so a reader can move from one site to the other.

| Section | Holds |
|---|---|
| Overview | What the product is and where it stands (vision, MVP) |
| Getting started | Run it locally; first steps |
| Concepts | The ideas a reader needs before the architecture |
| Architecture | Components, flows, lifecycles |
| Guides | One page per task (plans and how-tos) |
| API | The OpenAPI document, rendered |
| Reference | Config, environment, errors, extension contracts |
| Decisions | The ADR index and every ADR |
| Contributing | How to contribute, security, roadmap |

Rules: the sidebar is built in `.vitepress/config.ts` (folders list their pages by first heading); a page moves only
when the structure needs it, and then every link is fixed; Mermaid renders and local search is on; `ignoreDeadLinks`
is on because pages link to source files outside `docs/`, and `tools/docs-check` checks the links that stay inside
the repository; the base path is `/<repository>/` (`DOCS_BASE` overrides it).
