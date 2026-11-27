# The shared structure of the documentation sites (v2)

another-agentic-system, another-agentic-platform and another-adam-rs publish one VitePress site each, built from the
repository's own `docs/` with `@vaam-apps/vitepress-md3`, so a pull request updates its docs in the same change. All use
the same seven sections, in this order, and the same groups inside Guides and Reference, so a reader can move from one
site to another. The other repositories copy this file and never diverge.

## Sections

| # | Section | Holds |
|---|---|---|
| 1 | Overview | What the product is, where it stands, a picture (vision, MVP, requirements, open questions), and the group **Plans and checks** |
| 2 | Get started | Run it locally; the first success |
| 3 | Guides | One task per page, in four fixed groups (below) |
| 4 | Reference | The contracts, in fixed groups (below) |
| 5 | Architecture | Opens with **Concepts**, then components and lifecycles, as Mermaid pairs |
| 6 | Decisions | The ADR index and every ADR, listed and never rewritten |
| 7 | Contributing | How to build, test and open a pull request; security; roadmap |

## Groups

**Guides** has four groups: **Deploy**, **Operate**, **Use** and **Extend**. A guide is a how-to for a task. Nothing under
Get started or Guides is a plan, a check or a draft: those go to Overview, in **Plans and checks**.

**Reference** has the groups **API** (the OpenAPI document rendered, and the other contracts a client calls),
**Extensions** (the extension contracts), **Configuration** (the configuration file, environment variables and what a
deployment must honour) and **Errors**. A repository adds **CRDs**, **CLI** and **Rust API** when it has them.

A group with no page is not drawn. It appears when its first page does, and its name does not change.

**Architecture** opens with a **Concepts** page: the words the documentation uses, each in a line or two with a link to
the page that says the rest.

## Diagrams

Mermaid diagrams are drawn by `vitepress-mermaid-renderer` 1.2.3 (verified 2026-11-27: npm, `peerDependencies`
`mermaid` `^11 || ^12`, `@mermaid-js/mermaid-zenuml` optional) with `mermaid` 11.17.2, both pinned exactly in
`docs/package.json`. It replaces `vitepress-plugin-mermaid`, which has had no release since 2024-09. It adds zoom, pan,
fullscreen, copy and download to each diagram, and the diagrams are drawn in the theme's Material 3 colours, redrawn when
the scheme changes. Copy `docs/.vitepress/theme/mermaid.ts` (the layout that starts the renderer; `theme/index.ts` only
names it as `Layout`) and `docs/.vitepress/theme/mermaid.css` into a site that uses `@vaam-apps/vitepress-md3`, and
nothing else: `config.ts` has no Mermaid setting and needs no tag in `escapeUnknownTags`, because the renderer works on
the page in the browser.

## Sites

| Repository | Site | State |
|---|---|---|
| another-agentic-system | https://vymalo.github.io/another-agentic-system/ | live (answered HTTP 200 on 2026-11-27) |
| another-agentic-platform | https://vymalo.github.io/another-agentic-platform/ | live (answered HTTP 200 on 2026-11-27; it has its own `docs/.vitepress/` and a docs workflow) |
| another-adam-rs | https://vymalo.github.io/another-adam-rs/ | live (answered HTTP 200 on 2026-11-27; it has its own `docs/.vitepress/` and a docs workflow) |

## Agent docs

`.agents/docs/` (the documents written for agents, not for readers) is never in the sidebar and never on the site.

## Rules

The sidebar is built in `.vitepress/config.ts` (a folder lists its pages by first heading); existing pages are
mapped into sections and groups and not moved; a page moves only when the structure needs it, and then every link is
fixed; a draft stays in the repository and leaves the site through `srcExclude`; Mermaid renders and local search is on;
the build fails on a dead link, and only links to source files outside `docs/` (which `tools/docs-check` checks) are let
through by pattern; the base path is `/<repository>/` (`DOCS_BASE` overrides it).
