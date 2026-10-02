/*
 * The checks the orchestrator makes of `forwardedProps["vymalo.mentions"]` before it writes a message
 * (ADR 0026, docs/api/mentions-v1.md section 3, docs/api/agui.md "Mentions"), as the mock plays them. Offsets
 * count UTF-16 code units, which is what a JavaScript string indexes, so `slice` is the whole check.
 * It is an independent reading of the contract, not the web's own code: the web's tests then run
 * against it, and a mistake in one is not hidden by the same mistake in the other.
 */

export type MentionRef = {
  agentId: string;
  label: string;
  start: number;
  end: number;
  cardUrl?: string;
};

/** A problem the orchestrator would answer with: nothing was written. */
export type Refusal = { status: 400 | 422 | 503; title: string; detail: string };

/** At most this many references on one message. */
export const MAX_MENTIONS = 16;

const MEMBERS = new Set(["agentId", "label", "start", "end", "cardUrl"]);
const AGENT_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;

const bad = (detail: string): Refusal => ({ status: 400, title: "Invalid request", detail });
const unprocessable = (detail: string): Refusal => ({
  status: 422,
  title: "Unprocessable",
  detail,
});

/**
 * The shape, read on every run (400): absent or `null` is none; else an array of at most 16
 * objects with exactly the members of a reference, of the right types.
 */
export function readMentions(value: unknown): MentionRef[] | Refusal {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value)) return bad('forwardedProps["vymalo.mentions"] must be an array');
  if (value.length > MAX_MENTIONS) {
    return bad(`forwardedProps["vymalo.mentions"] holds at most ${MAX_MENTIONS} references`);
  }
  const out: MentionRef[] = [];
  for (const [i, item] of value.entries()) {
    if (typeof item !== "object" || item === null || Array.isArray(item)) {
      return bad(`vymalo.mentions[${i}] must be an object`);
    }
    const r = item as Record<string, unknown>;
    for (const key of Object.keys(r)) {
      if (!MEMBERS.has(key)) return bad(`vymalo.mentions[${i}] has an unknown member "${key}"`);
    }
    if (typeof r.agentId !== "string" || typeof r.label !== "string") {
      return bad(`vymalo.mentions[${i}] needs a string agentId and label`);
    }
    if (!Number.isInteger(r.start) || !Number.isInteger(r.end)) {
      return bad(`vymalo.mentions[${i}] needs integer start and end`);
    }
    if (r.cardUrl !== undefined && typeof r.cardUrl !== "string") {
      return bad(`vymalo.mentions[${i}].cardUrl must be a string`);
    }
    out.push({
      agentId: r.agentId,
      label: r.label,
      start: r.start as number,
      end: r.end as number,
      ...(typeof r.cardUrl === "string" ? { cardUrl: r.cardUrl } : {}),
    });
  }
  return out;
}

const isHigh = (u: number) => u >= 0xd800 && u <= 0xdbff;
const isLow = (u: number) => u >= 0xdc00 && u <= 0xdfff;

/**
 * The references against the text (422): a label is `@` and 1 to 63 more units, and is the text at
 * its offsets; `start < end <= length`; no offset inside a surrogate pair; sorted by `start`, no
 * overlap.
 */
export function checkAgainstText(text: string, refs: readonly MentionRef[]): Refusal | undefined {
  let at = 0;
  for (const [i, m] of refs.entries()) {
    const where = `vymalo.mentions[${i}]`;
    if (!m.label.startsWith("@") || m.label.length < 2 || m.label.length > 64) {
      return unprocessable(`${where}: a label is "@" and 1 to 63 more characters`);
    }
    if (m.start < 0 || m.start >= m.end || m.end > text.length) {
      return unprocessable(
        `${where}: start and end must satisfy 0 <= start < end <= ${text.length}`,
      );
    }
    for (const offset of [m.start, m.end]) {
      if (offset > 0 && offset < text.length) {
        if (isHigh(text.charCodeAt(offset - 1)) && isLow(text.charCodeAt(offset))) {
          return unprocessable(`${where}: an offset must not fall inside a surrogate pair`);
        }
      }
    }
    if (text.slice(m.start, m.end) !== m.label) {
      return unprocessable(`${where}: the text at ${m.start}..${m.end} is not "${m.label}"`);
    }
    if (m.start < at) {
      return unprocessable(`${where}: references must be sorted by start and must not overlap`);
    }
    at = m.end;
  }
  return undefined;
}

/** What the agent checks need to know of the deployment. */
export type Lookup = {
  /** The agent that reads the message: it cannot be mentioned in its own thread. */
  own: string;
  /** Whether the person's roles let them invoke an agent (`agent.invoke`). */
  mayInvoke: (agentId: string) => boolean;
  /**
   * What the registry says of an agent: its card URL, `null` when it lists none for it,
   * `undefined` when it does not know the agent, `"unavailable"` when it cannot answer.
   */
  card: (agentId: string) => string | null | undefined | "unavailable";
};

/**
 * The references against the registry and the person's roles (422, 503), for each reference in
 * order. The roles are asked before the registry, so a person is never told whether an agent they
 * may not invoke exists.
 */
export function checkAgents(refs: readonly MentionRef[], lookup: Lookup): Refusal | undefined {
  for (const m of refs) {
    if (!lookup.mayInvoke(m.agentId)) return unprocessable(`you may not use '${m.agentId}'`);
    if (m.agentId === lookup.own) {
      return unprocessable("an agent cannot be mentioned in its own thread");
    }
    if (!AGENT_ID.test(m.agentId)) return unprocessable(`unknown agent '${m.agentId}' in mentions`);
    const card = lookup.card(m.agentId);
    if (card === "unavailable") {
      return {
        status: 503,
        title: "Service unavailable",
        detail: "the agent registry could not answer; try again",
      };
    }
    if (card === undefined) return unprocessable(`unknown agent '${m.agentId}' in mentions`);
    if (m.cardUrl !== undefined && m.cardUrl !== card) {
      return unprocessable(`the card of '${m.agentId}' moved; refresh the agent list`);
    }
  }
  return undefined;
}
