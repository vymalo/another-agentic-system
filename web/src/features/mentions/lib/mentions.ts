import type { ApiAgent, ApiMention } from "@/lib/api/types";

export { MENTIONS_KEY, MENTIONS_PROP, MENTIONS_URI } from "@/features/chat/lib/agui/vymalo";

/*
 * Mentions as the composer holds them (ADR 0026, docs/api/mentions-v1.md): the agents a message
 * names, as structured references into its text. Everything here is pure and counts in UTF-16 code
 * units, what a JavaScript string indexes (`slice`, `length`, `selectionStart`): that is the
 * unit of the contract, so no conversion stands between the text a person edits and the offsets
 * that are sent. Nothing here is a check the orchestrator does not repeat (it refuses a bad label,
 * an unknown agent and one the person may not invoke); the screen only never sends what it knows
 * would be refused, and says what it dropped by not drawing it.
 */

/** A reference as it is sent: `forwardedProps["vymalo.mentions"]` is a list of these. */
export type Mention = ApiMention;

/** At most this many references on one message (the orchestrator answers 400 for more). */
export const MAX_MENTIONS = 16;

/** The label of a mention, at most this many UTF-16 code units: `@` and the agent's id. */
export const MAX_LABEL = 64;

/**
 * The text a mention of the agent shows: `@` and its id. An id is what the registry keeps an agent
 * under (`^[a-z0-9][a-z0-9-]{0,62}$`): it has no space, so the label is one word that ends where
 * the person's next word begins, it never collides with another agent's, and it is at most 64 code
 * units by construction. A display name has none of these properties.
 */
export const labelOf = (agentId: string): string => `@${agentId}`;

/** Whether an agent id can be written as a label the orchestrator accepts. */
export const mentionable = (agentId: string): boolean => labelOf(agentId).length <= MAX_LABEL;

/** Characters an agent id is made of: a label is over where the next one that is not of them is. */
const HANDLE = /[A-Za-z0-9_-]/;

const isHigh = (unit: number): boolean => unit >= 0xd800 && unit <= 0xdbff;
const isLow = (unit: number): boolean => unit >= 0xdc00 && unit <= 0xdfff;

/** Whether `at` falls between the two halves of a surrogate pair of `text`. */
export const insidePair = (text: string, at: number): boolean =>
  at > 0 && at < text.length && isHigh(text.charCodeAt(at - 1)) && isLow(text.charCodeAt(at));

/**
 * Whether a mention still stands in `text`: its label is the text at its offsets, it begins a word
 * (the start of the text or after white space) and ends one (the end of the text or before a
 * character an agent id is not made of). A person who types "x" right after "@coder" has made a
 * different word, "@coderx", and the mention is gone: the same rule that drops one whose label was
 * edited. It is stricter than the orchestrator's (which checks the label at the offsets only), so
 * what passes here is never refused for it.
 */
export function stands(text: string, m: Mention): boolean {
  if (m.start < 0 || m.end > text.length || m.start >= m.end) return false;
  if (text.slice(m.start, m.end) !== m.label) return false;
  if (insidePair(text, m.start) || insidePair(text, m.end)) return false;
  const before = m.start === 0 ? "" : (text[m.start - 1] ?? "");
  const after = text[m.end] ?? "";
  if (before !== "" && !/\s/.test(before)) return false;
  return after === "" || !HANDLE.test(after);
}

/**
 * Where `before` and `after` differ: the longest common prefix and, past it, the longest common
 * suffix, in code units, never splitting a surrogate pair (a pair that differs in its second half
 * only is one edited character, not half of one).
 */
function changed(before: string, after: string): { start: number; oldEnd: number; newEnd: number } {
  const limit = Math.min(before.length, after.length);
  let start = 0;
  while (start < limit && before.charCodeAt(start) === after.charCodeAt(start)) start++;
  if (start > 0 && isHigh(before.charCodeAt(start - 1))) start--;
  let tail = 0;
  while (
    tail < limit - start &&
    before.charCodeAt(before.length - 1 - tail) === after.charCodeAt(after.length - 1 - tail)
  ) {
    tail++;
  }
  // a suffix that begins on a low surrogate would leave its high half in the edit: give it back
  if (tail > 0 && isLow(before.charCodeAt(before.length - tail))) tail--;
  return { start, oldEnd: before.length - tail, newEnd: after.length - tail };
}

/**
 * The mentions after the text went from `before` to `after`: those the edit did not touch, moved
 * by what it added or removed in front of them; a mention the edit overlaps, a label edited away or
 * one that no longer begins and ends a word, is dropped. The edit is found by comparing the two
 * texts, so it does not matter how the text changed (typing, a paste over a selection, an undo, the
 * runtime clearing the box). Offsets are UTF-16 code units, and the result is sorted by `start`.
 */
export function reconcile(before: string, after: string, mentions: readonly Mention[]): Mention[] {
  if (before === after) return mentions.filter((m) => stands(after, m));
  if (before.trim() === after.trim()) {
    // the same words with white space added or cut at the ends (the runtime trims what it sends): move by the front's
    const lead =
      after.length - after.trimStart().length - (before.length - before.trimStart().length);
    return standing(
      after,
      mentions.map((m) => ({ ...m, start: m.start + lead, end: m.end + lead })),
    );
  }
  const { start, oldEnd, newEnd } = changed(before, after);
  const delta = newEnd - oldEnd;
  const kept: Mention[] = [];
  for (const m of mentions) {
    let next: Mention;
    if (m.end <= start) next = m;
    else if (m.start >= oldEnd) next = { ...m, start: m.start + delta, end: m.end + delta };
    else continue; // the edit is inside the label, or across its edge
    if (stands(after, next)) kept.push(next);
  }
  return kept.sort((a, b) => a.start - b.start);
}

/**
 * The mentions that still stand in `text`, in order, without overlaps, at most `MAX_MENTIONS`: what
 * a message may carry. `[]` when nothing is mentioned.
 */
export function standing(text: string, mentions: readonly Mention[]): Mention[] {
  const sorted = mentions.filter((m) => stands(text, m)).sort((a, b) => a.start - b.start);
  const out: Mention[] = [];
  let at = 0;
  for (const m of sorted) {
    if (m.start < at) continue; // overlaps the one before
    out.push(m);
    at = m.end;
  }
  return out.slice(0, MAX_MENTIONS);
}

/**
 * The mentions of `text` for a message that goes out trimmed (the composer sends `trim()`ed text):
 * moved past the white space that was cut off the front, and only the ones that still stand there.
 */
export function forTrimmed(text: string, mentions: readonly Mention[]): Mention[] {
  const lead = text.length - text.trimStart().length;
  return standing(
    text.trim(),
    mentions.map((m) => ({ ...m, start: m.start - lead, end: m.end - lead })),
  );
}

/** What is being typed after an "@": where it begins (the `@`) and what follows it up to the caret. */
export type Trigger = { start: number; query: string };

/**
 * The mention being typed at `caret`: an "@" that begins a word (the start of the text or after white
 * space, so an e-mail address is not one) followed by characters that are not white space or
 * another "@", up to the caret. Null when the caret is not at the end of such a word.
 */
export function triggerAt(text: string, caret: number): Trigger | null {
  if (caret < 0 || caret > text.length) return null;
  const upTo = text.slice(0, caret);
  const match = /(^|\s)@([^\s@]*)$/.exec(upTo);
  if (!match) return null;
  const query = match[2] ?? "";
  return { start: caret - query.length - 1, query };
}

/** The agents a typed query matches: by the start of the id, of the name or of a word of the name. */
export function matching(agents: readonly ApiAgent[], query: string): ApiAgent[] {
  const q = query.toLowerCase();
  if (q === "") return [...agents];
  const starts = (s: string) =>
    s
      .toLowerCase()
      .split(/[\s\-_/]+/)
      .some((word) => word.startsWith(q));
  return agents.filter((a) => a.id.toLowerCase().startsWith(q) || starts(a.name) || starts(a.id));
}

/** A mention of `agent` written in place of the "@query" the person was typing. */
export type Inserted = { text: string; caret: number; mention: Mention };

/**
 * The text with the typed "@query" replaced by the agent's label and a space (none when white space
 * already follows), the caret after it, and the mention for it. `others` are the mentions the text
 * already had; the caller moves them with `reconcile(text, inserted.text, others)`.
 */
export function insertMention(
  text: string,
  trigger: Trigger,
  caret: number,
  agent: ApiAgent,
): Inserted {
  const label = labelOf(agent.id);
  const rest = text.slice(caret);
  const space = rest === "" || !/^\s/.test(rest) ? " " : "";
  const next = `${text.slice(0, trigger.start)}${label}${space}${rest}`;
  const start = trigger.start;
  const mention: Mention = {
    agentId: agent.id,
    label,
    start,
    end: start + label.length,
    ...(agent.cardUrl ? { cardUrl: agent.cardUrl } : {}),
  };
  return { text: next, caret: start + label.length + space.length, mention };
}

/** `text` without the label of `mention` (and the space after it that the pick added), for its chip's remove button. */
export function withoutMention(text: string, mention: Mention): string {
  const after = text[mention.end] === " " ? mention.end + 1 : mention.end;
  return `${text.slice(0, mention.start)}${text.slice(after)}`;
}

/**
 * The references a log carries (`metadata["vymalo.mentions"]` of a user message), read defensively:
 * the ones of the right shape, and, given the message `text`, only where the text says what the
 * reference says (a stale one is not drawn). Sorted by `start`.
 */
export function parseMentions(value: unknown, text?: string): Mention[] {
  if (!Array.isArray(value)) return [];
  const out: Mention[] = [];
  for (const item of value) {
    if (typeof item !== "object" || item === null) continue;
    const r = item as Record<string, unknown>;
    if (
      typeof r.agentId !== "string" ||
      typeof r.label !== "string" ||
      typeof r.start !== "number" ||
      typeof r.end !== "number" ||
      !Number.isInteger(r.start) ||
      !Number.isInteger(r.end) ||
      r.start < 0 ||
      r.end <= r.start
    ) {
      continue;
    }
    if (text !== undefined && text.slice(r.start, r.end) !== r.label) continue;
    out.push({
      agentId: r.agentId,
      label: r.label,
      start: r.start,
      end: r.end,
      ...(typeof r.cardUrl === "string" ? { cardUrl: r.cardUrl } : {}),
    });
  }
  return out.sort((a, b) => a.start - b.start);
}

/** The text of a message cut at its mentions: the words between them and the mentions themselves. */
export type Segment = { text: string } | { text: string; mention: Mention };

export function segments(text: string, mentions: readonly Mention[]): Segment[] {
  const out: Segment[] = [];
  let at = 0;
  for (const m of mentions) {
    if (m.start < at) continue;
    if (m.start > at) out.push({ text: text.slice(at, m.start) });
    out.push({ text: text.slice(m.start, m.end), mention: m });
    at = m.end;
  }
  if (at < text.length) out.push({ text: text.slice(at) });
  return out;
}
