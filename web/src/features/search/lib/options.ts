import type { ApiSearchHit, ApiThread, ApiThreadHit } from "@/lib/api/types";

/** What the person can open from the results: one hit of one thread. The list of them, in order, is what the arrow keys walk. */
export type SearchOption = {
  /** The element id (`aria-activedescendant` names it). */
  id: string;
  thread: ApiThread;
  hit: ApiSearchHit;
  /** Where it opens: a message hit at its turn (`#m-<seq>`), a title or a description hit at the thread. */
  href: string;
};

/** Where a hit opens. A message hit carries the `seq` to open the thread at (docs/api/thread-search-v1.md, "Client recipe"). */
export function hrefOf(thread: ApiThread, hit: ApiSearchHit): string {
  const base = `/threads/${encodeURIComponent(thread.id)}`;
  return hit.field === "message" && typeof hit.seq === "number" ? `${base}#m-${hit.seq}` : base;
}

/** Every hit of every result, flat, in the order the page draws them. */
export function optionsOf(results: ApiThreadHit[], prefix: string): SearchOption[] {
  const options: SearchOption[] = [];
  results.forEach((result, r) => {
    result.hits.forEach((hit, h) => {
      options.push({
        id: `${prefix}-${r}-${h}`,
        thread: result.thread,
        hit,
        href: hrefOf(result.thread, hit),
      });
    });
  });
  return options;
}

/** The index after a press of an arrow key: wraps at both ends; from nothing, Down is the first and Up the last. */
export function step(current: number, delta: 1 | -1, count: number): number {
  if (count === 0) return -1;
  if (current < 0) return delta === 1 ? 0 : count - 1;
  return (current + delta + count) % count;
}

/** The words of a query, as the server reads them first (letters, marks, numbers): used only to say that more than the limit were typed. */
export function wordCount(text: string): number {
  return text.match(/[\p{L}\p{M}\p{N}]+/gu)?.length ?? 0;
}

/** Blank as the contract has it: nothing but U+0020, U+0009, U+000A and U+000D. */
export function isBlank(text: string): boolean {
  return /^[ \t\n\r]*$/.test(text);
}

/** The first `max` Unicode scalar values of `text` (the server counts scalar values, not UTF-16 units). */
export function clip(text: string, max: number): string {
  const chars = Array.from(text);
  return chars.length <= max ? text : chars.slice(0, max).join("");
}
