import "katex/dist/katex.min.css";
import rehypeKatex from "rehype-katex";
import remarkMath from "remark-math";
import { remarkDisplayMath } from "./remark-display-math";

/*
 * Math in an answer (web/README.md "Code and math in answers"): display math only, `$$` on lines of its own
 * (`remark-display-math.ts` holds the rule and the size limit). This module is its own chunk (KaTeX and its stylesheet and
 * fonts are 77 KB gzipped, and most threads have no formula): `markdown-text.tsx` imports it when the text has a line
 * that starts with `$$`, and until then the text is text.
 *
 * A single dollar is NOT math (`singleDollarTextMath: false`), so "$5 or $10" is a price.
 *
 * The KaTeX settings below are the security controls, held by `markdown-text.dom.test.tsx` ("KaTeX trusts nothing", "a
 * macro bomb is cut"): `trust: false` refuses `\href`, `\url`, `\includegraphics`, `\htmlId`, `\htmlClass`, `\htmlStyle` and
 * `\htmlData` (the formula could otherwise make links, load images and set ids, classes and styles on the page), and
 * `maxExpand` stops a macro that expands without end (`\def\a{\a\a}\a`) after that many expansions. `maxSize` bounds
 * what a formula can ask of a length. `rehype-katex` sets `throwOnError` and, for a formula KaTeX cannot read, `strict`
 * itself (it asks KaTeX to throw, then draws the source with `throwOnError: false`), so neither is set here. A formula
 * that cannot be read is drawn as its source in `errorColor`. KaTeX's fonts are bundled, so the CSP's `font-src 'self'`
 * holds.
 */

export const remarkPlugins: unknown[] = [
  [remarkMath, { singleDollarTextMath: false }],
  remarkDisplayMath,
];

export const rehypePlugins: unknown[] = [
  [
    rehypeKatex,
    {
      // the page's own error colour, which is checked in both schemes (KaTeX's #cc0000 is not on the dark one)
      errorColor: "var(--destructive)",
      // no warning in the console for what a model writes that LaTeX itself would accept
      strict: "ignore",
      trust: false,
      maxSize: 20,
      maxExpand: 1000,
    },
  ],
];
