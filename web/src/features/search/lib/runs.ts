import type { ApiSearchSnippet } from "@/lib/api/types";

/** A piece of a snippet: plain, or one of the words that matched (drawn in a `<mark>`). */
export type Run = {
  text: string;
  mark: boolean /** Where it starts in the text: a key that is stable. */;
  at: number;
};

/**
 * The runs of a snippet: its `text` cut at `matches`. The offsets are UTF-16 code units into `text` (docs/api/thread-search-v1.md),
 * which is what `String.prototype.slice` counts, so no conversion is made. The server never cuts inside a surrogate pair; a match that is
 * out of order, empty, outside the text or inside a pair is skipped here instead of being drawn wrongly: the text is always drawn whole.
 * Nothing here is markup: every run is a string for a text node.
 */
export function runsOf({ text, matches }: Pick<ApiSearchSnippet, "text" | "matches">): Run[] {
  const runs: Run[] = [];
  let at = 0;
  for (const { start, end } of matches) {
    if (!Number.isInteger(start) || !Number.isInteger(end)) continue;
    if (start < at || end <= start || end > text.length) continue;
    if (splitsPair(text, start) || splitsPair(text, end)) continue;
    if (start > at) runs.push({ text: text.slice(at, start), mark: false, at });
    runs.push({ text: text.slice(start, end), mark: true, at: start });
    at = end;
  }
  if (at < text.length) runs.push({ text: text.slice(at), mark: false, at });
  return runs;
}

/** Whether `index` is between the two halves of a surrogate pair. */
function splitsPair(text: string, index: number): boolean {
  if (index <= 0 || index >= text.length) return false;
  const before = text.charCodeAt(index - 1);
  const after = text.charCodeAt(index);
  return before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff;
}
