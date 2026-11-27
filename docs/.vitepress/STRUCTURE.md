# The shared structure of the documentation sites (v3)

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

## Writing pages

A page is read in layers, so it is progressive: the reader stops when they have enough.

- **Lead with what a reader needs, in a few lines.** The first screen says what the thing is and what to do. Nothing else
  is needed to use it.
- **Put the rest in `::: in-depth`.** The detail, the history, the edge cases and the long tables go in a block that is
  closed until opened. Give it a plain-text title (no Markdown, no HTML) that says what is inside (`::: in-depth Why a row that dies is never lost`);
  with no title it is "In depth". Blocks nest (`::: in-depth` inside `::: in-depth`, each closed by its own `:::`),
  and a code fence inside is left alone. Inside VitePress's own `::: tip` or `::: details`, write that outer marker longer (`:::: tip` … `::::`), or the tip closes at the block's `:::`. It is a native `<details>`, so the keyboard and a screen reader work with no
  script. A block cannot sit inside a table cell: use a list or headings, a line each, then the block. Like `::: tip`,
  `::: in-depth` shows as literal text when the file is read on GitHub.
- **Status tables use the legend below.** The icon always comes with its word in the cell (`✅ built`), because an icon
  alone says nothing to a screen reader or to a reader who does not know it. The cell keeps its facts after the word.

| Icon | Word | Means |
|---|---|---|
| ✅ | built | fully supported or built |
| ⚠️ | partial | partially: the cell says what is missing |
| ❌ | not supported | not supported |
| 🚧 | planned | in progress or planned |
| 🧪 | experimental | works, and may change or be removed |
| ➖ | n/a · retired | not applicable, or retired |
| 🔒 | needs a role | needs a permission or a role |

The legend is also on the site's reference index, under "Status icons", with the same text. The icons are literal emoji in the
Markdown, not shortcodes, so they read the same on GitHub.

### The pieces to copy

Two files, in `docs/.vitepress/theme/`, and two lines:

- `in-depth.ts`: the container, a small markdown-it block rule with no dependency. In `config.ts`, import it and call it
  beside any other markdown hook, so that your own hooks stay only what they are:
  `markdown: { config: (md) => { md.use(inDepth); yourOwnHook(md); } }`. An unclosed block logs a build warning naming
  the file and line.
- `extras.css`: the sidebar width and the block's style (both colour schemes, from the md3 tokens). In `theme/index.ts`,
  `import "./extras.css"`.

Optional, and worth copying with them: `in-depth.test.mjs` (`node --test`, run by CI) and the check in
`tools/docs-check/check-docs.mjs` that fails on an unclosed `::: in-depth`.

**The sidebar** is 320px wide from 960px up and 360px from 1440px up, where the layout is capped (`--vp-sidebar-width`,
272px by default): VitePress derives the content's offset, the local nav and the fixed sidebar of wide screens from that
one variable, so they move together. Under 960px the sidebar is a drawer and is not changed. At 1280px and 1303px the
content column and the outline sit side by side, the outline ending inside the viewport.

**For agents that read the site:** it publishes `llms.txt`, `llms-full.txt` and a `.md` per page with
`vitepress-plugin-llms` 1.15.0 (pinned, `docs/package.json`; a Vite plugin in `vite.plugins`, no network at build time).
Copy the plugin call in `config.ts` and `llms.ts` (it lifts the sidebar's nested groups so that the links keep `base`, and
leaves out the dynamic routes of the API). The plugin throws if the site has no index page.

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
