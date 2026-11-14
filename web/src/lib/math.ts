import "katex/dist/katex.min.css";
import rehypeKatex from "rehype-katex";
import remarkMath from "remark-math";
import { rehypeMathLabel } from "./rehype-math-label";

/*
 * Math in an answer (web/README.md "Code and math in answers"): `$$...$$`, inline or as a block. This module is its own chunk
 * (KaTeX and its stylesheet and fonts are 77 KB gzipped, and most threads have no formula): `markdown-text.tsx` imports it
 * when the text holds `$$`, and until then the text is text.
 *
 * A single dollar is NOT math (`singleDollarTextMath: false`), so "$5 or $10" is a price. KaTeX never throws on a bad
 * formula (it draws the source in the error colour), trusts nothing (no `\href`, no `\includegraphics`), and cuts a
 * formula that expands without end at `maxExpand`. Its fonts are bundled, so the CSP's `font-src 'self'` holds.
 */

export const remarkPlugins: unknown[] = [[remarkMath, { singleDollarTextMath: false }]];

export const rehypePlugins: unknown[] = [
  [
    rehypeKatex,
    {
      throwOnError: false,
      // the page's own error colour, which is checked in both schemes (KaTeX's #cc0000 is not on the dark one)
      errorColor: "var(--destructive)",
      strict: "ignore",
      trust: false,
      maxSize: 20,
      maxExpand: 1000,
    },
  ],
  rehypeMathLabel,
];
