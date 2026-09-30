/**
 * Reading the text of a verification card (ADR 0018). Findings, summaries, names and commits come
 * from a tool, a CI provider or a reviewer: they are untrusted, and the only thing done to them
 * here is to shorten them. They are drawn as text by the renderers, never as markup.
 */

const SOURCE_LABELS: Record<string, string> = {
  ci: "CI",
  agent_checks: "Agent checks",
  verifier: "Verifier",
};

/** A source the UI knows by its label, an unknown one (a newer orchestrator) by its own name, shortened. */
export function sourceLabel(source: string): string {
  return SOURCE_LABELS[source] ?? truncate(source.replaceAll("_", " "), 40).text;
}

const HEX_COMMIT = /^[0-9a-f]{7,64}$/i;

/** `0000001` for a full commit hash; anything else (it is untrusted) is cut to 12 characters. */
export function shortCommit(commit: string): string {
  return HEX_COMMIT.test(commit) ? commit.slice(0, 7) : truncate(commit, 12).text;
}

/**
 * At most `max` UTF-16 units of `text` (never half of a surrogate pair), with an ellipsis when
 * something was cut. `cut` says whether it was.
 */
export function truncate(text: string, max: number): { text: string; cut: boolean } {
  if (text.length <= max) return { text, cut: false };
  let end = max;
  const last = text.charCodeAt(end - 1);
  if (last >= 0xd800 && last <= 0xdbff) end -= 1; // the high half of a pair
  return { text: `${text.slice(0, end).trimEnd()}…`, cut: true };
}

/** Each string with a key that is stable and unique: the text plus how often it came before. */
export function keyed(items: readonly string[]): { key: string; text: string }[] {
  const seen = new Map<string, number>();
  return items.map((text) => {
    const n = seen.get(text) ?? 0;
    seen.set(text, n + 1);
    return { key: `${n}:${text}`, text };
  });
}

export const pluralFindings = (n: number): string => `${n} ${n === 1 ? "finding" : "findings"}`;
