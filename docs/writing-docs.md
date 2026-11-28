# Writing docs

A change updates its docs in the same pull request.

## Rules

- **Keep pages short.** Lead with what the reader needs. Put the rest in an `::: in-depth` block.
- **A decision is an ADR.** Add `docs/decisions/NNNN-kebab-title.md` with the next number, and a row in the Decisions
  table of the repository's `README.md`. Change a past decision with a dated amendment or a superseding ADR, never by
  rewriting it. See the [ADR index](decisions/index.md).
- **A process is a Mermaid pair**, then prose: a `sequenceDiagram` for the interaction and a `stateDiagram-v2` for the
  lifecycle.
- **Mark facts** as *verified*, with the date and the source, or as *unverified*.
- **Screenshots are the web's own.** `pnpm screens`, run in `web/`, writes them to `web/e2e/__screens__/`. Link those
  files and never copy them. Use a `<picture>` of the light and the dark file, alt text that says what the screen shows,
  and a caption that says it comes from the web's mock server.
- **Open questions are never deleted.** They move between Open, Closed and Moved in
  [Open questions](open-questions.md).

The gate is `tools/docs-check`: every diagram parses, and every relative link and image resolves. Its commands are in
[CONTRIBUTING.md](../CONTRIBUTING.md#set-up-build-and-test).

## Run the site

Run these from the root of the repository.

```sh [terminal]
npm --prefix docs ci
npm --prefix docs run dev
```

Before you open a pull request, build the site and check its links.

```sh [terminal]
npm --prefix docs run build
npm --prefix docs run check:links
```

## Where a page goes

[STRUCTURE.md](../docs/.vitepress/STRUCTURE.md) decides the sections and groups of the site, how a page is written,
how long a sidebar label may be, and how links work.
