# orch-svg-clean

A pure, allow-list sanitizer for SVG: what an image of an agent may keep to be served inline.

## Where it sits

A pure helper of the artifact path ([ADR 0032](../../../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md),
decision 9). An agent can hand a person an SVG; a browser runs the script an SVG holds when the document is opened as
a page. The API ([`orch-api`](../api/README.md)) therefore passes an SVG through `clean` before it serves one
**inline**; a download is the original bytes, as an attachment, and the web draws a file only as `<img src>`, where an
SVG runs no script and loads no subresource. This is the layer under both. No I/O, no async, one dependency
(`quick-xml`, already in the lock file through the S3 store's `object_store`).

## API at a glance

| Item | What |
|---|---|
| `clean(&[u8]) -> Result<Vec<u8>, SvgError>` | reads the SVG as XML and writes it again from **what the lists allow and nothing else** (below). Stable: `clean(clean(x)) == clean(x)` |
| `SvgError` | `Malformed` (not well-formed XML, not UTF-8, a duplicate attribute), `TooDeep` (more than `MAX_DEPTH`, 64, levels) and `NotSvg` (no `svg` root that can be kept). The caller does not serve such a file inline |
| `ELEMENTS`, `ATTRIBUTES`, `MAX_DEPTH` | the allow-lists and the bound, public so a test can check an output against them |

## What is kept, what is dropped

* **Elements** in `ELEMENTS`: shapes, text, gradients, patterns, clipping and masks, markers, the filter primitives that
  load nothing, `style` and `use`. Anything else is dropped **with its whole subtree**: `script`, `foreignObject`,
  `iframe`, `a`, `image`, `feImage`, the animation elements (`animate` or `set` can write an `href` that `javascript:`
  fills), every element that has a prefix (another namespace), and any other name (names are case sensitive:
  `<SCRIPT>` is not on the list).
* **Attributes** in `ATTRIBUTES`: geometry, presentation, the gradient and filter parameters. Every `on*` attribute is
  not on it. `href` and `xlink:href` stay only on `use`, the gradients, `pattern`, `textPath` and `filter`, and only when
  the value (after its entities are read, so `&#106;avascript:` is no way round) **starts with `#`**; a `use` with no
  such reference is dropped. A `style` attribute is dropped when it holds `url(`, `@import`, `expression(`,
  `javascript:`, `behavior` or `-moz-binding`, comments removed first, or a backslash (a CSS escape can spell any of
  them). Any other attribute whose value has a `url(` keeps it only for `url(#id)` of the same document. `xmlns` and
  `xmlns:xlink` stay only with their own namespaces. An attribute that names an entity the document does not define
  (there is no DOCTYPE) is dropped.
* **Text** is kept where the element keeps text (`text`, `tspan`, `textPath`, `title`, `desc`, `style`, and blanks
  elsewhere) and is escaped again by this crate, never copied. A CDATA section is kept as escaped text. A `style`
  element is kept only if its text passes the rule of the `style` attribute, and drops any element inside it.
* **Never kept:** comments, processing instructions, the XML declaration, a DOCTYPE (so no entity is ever defined and a
  reference to one is dropped, which is what stops external entities and "billion laughs"), anything after the root.

## Tests

`tests/corpus.rs`: a corpus of 48 hostile payloads (the SVG ones of the OWASP XSS filter evasion lists and the usual XML
attacks: scripts in every spelling, handlers on every element, `foreignObject` with an iframe, javascript links
(entity-encoded, with a tab), `use` of remote and data documents, animation writing an `href` or a handler, images,
`url()` in `style` attributes and stylesheets (an import, a comment inside a keyword, a CSS escape, a binding), remote
`fill` and `filter`, comments that close early, processing instructions, external and expanding entities, a billion
laughs, a second root, other namespaces), each cleaned and then held to the properties of every output (no marker of an
attack survives, a re-read of the output finds only elements and attributes of the lists and `href`s that start with
`#`, and cleaning it again changes nothing); the specific payloads gone with the drawing around them kept; ordinary
drawings (gradients, `use`, transforms, text with entities, a stylesheet, a filter) coming back byte for byte; every
input that is not an SVG an error; the nesting bound; and a flat drawing of 20 000 shapes. `src/lib.rs` has one unit
test.

```sh
cargo test -p orch-svg-clean
```
