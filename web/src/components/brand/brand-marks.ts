import type { Glyph } from "@/components/glyph";
import { GitHubMark } from "./github-mark";
import { OpenCodeMark } from "./opencode-mark";

/** The third-party marks the page bundles: each is listed in third-party-notices.md. */
export const BRAND_MARKS: readonly Glyph[] = [GitHubMark, OpenCodeMark];

/** Whether a glyph is a brand's mark, which keeps its neutral colour in every state of a step. */
export function isBrandMark(glyph: unknown): boolean {
  return BRAND_MARKS.includes(glyph as Glyph);
}
