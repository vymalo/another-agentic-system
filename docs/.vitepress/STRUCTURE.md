# The shared structure of the documentation sites

Every Another repository (another-agentic-system, another-agentic-platform, another-adam-rs) publishes a
VitePress site with `@vaam-apps/vitepress-md3`, and every site has **the same seven sections in the
same order**, so a reader can follow from one project to another. The platform and adam-rs copy this
file and never diverge from it; a change starts here.

| # | Section | What goes in it |
| --- | --- | --- |
| 1 | **Overview** | What the project is, its status, a picture |
| 2 | **Get started** | Run it locally; the first success |
| 3 | **Guides** | One task per page |
| 4 | **Reference** | The API (the OpenAPI document), and the config, CRDs, environment variables and errors |
| 5 | **Architecture** | Components and lifecycle, as Mermaid pairs |
| 6 | **Decisions** | The ADRs / ADs and the open questions, listed and never rewritten |
| 7 | **Contributing** | How to build, test and open a pull request |

## Rules

- **Existing pages are mapped, not rewritten.** The sidebar in `docs/.vitepress/config.ts` places each page in a
  section; files do not move, so `tools/docs-check` and every link into `docs/` keep working.
- **Same tooling.** `@vaam-apps/vitepress-md3` and VitePress are pinned to exact versions in `docs/package.json`;
  Mermaid renders (`vitepress-plugin-mermaid`); local search is on.
- **No dead links.** The build fails on a dead link (`ignoreDeadLinks` is never set). A link out of `docs/` (the
  code, a crate README) is rewritten to its GitHub URL at build time (the `repository_links` rule of the config).
- **Plain Markdown stays plain.** The pages are written for GitHub: `<name>` and `{{ x }}` in prose are text
  (the `literal_angle_brackets` rule and the text renderers of the config).
- **Test fixtures are not pages.** `docs/api/examples/**` is excluded from the site.
- **Published** to GitHub Pages by `.github/workflows/docs-site.yml` on every push to `main`; a pull request
  only builds.
