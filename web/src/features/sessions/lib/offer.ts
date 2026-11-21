/**
 * The subagents an agent offers (ADR 0072, `docs/api/child-sessions-v1.md` section 1): the helpers a
 * person may start a session with, and the most sessions open at once in one thread, read from the
 * agent's live card as the capabilities document relays it.
 *
 * The orchestrator reads the card and carries the offer under the extension's URI; this module reads
 * that key as strictly as the contract has it (the rules of the Rust reader,
 * `orch_core::child_sessions::ChildSessionsOffer::from_params`, and the contract's bound on
 * `maxOpen`), and nothing else. A card that lists the extension badly is a card that does not list it
 * (ADR 0008, fail closed): the result is no offer at all, never a shorter list.
 */

/** The A2A extension an agent lists when it runs its helpers as sessions. */
export const CHILD_SESSIONS_URI = "https://agents.vymalo.com/a2a/extensions/child-sessions/v1";

/** One helper: the agent's own name and description (untrusted card text, drawn as text). */
export type ChildAgent = { name: string; description: string };

export type ChildSessionsOffer = {
  /** The helpers a session can be started with, in the card's order; at least one, each name once. */
  agents: ChildAgent[];
  /** The most sessions open (working or idle) at once in one thread: 1 to 256. */
  maxOpen: number;
};

/** The most sessions one thread may note in its life: an offer above it cannot be honoured. */
const MOST_OPEN = 256;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * `custom[CHILD_SESSIONS_URI]` of the capabilities document: `agents` a non-empty array of
 * `{name, description}` (both strings, `name` not blank, each name once) and `maxOpen`, a whole number
 * from 1 to 256 (a whole double such as `16.0` is the same number in JavaScript, as an SDK hands it
 * back). Unknown members are ignored; anything else is `null`.
 */
export function parseChildSessionsOffer(value: unknown): ChildSessionsOffer | null {
  if (!isRecord(value)) return null;
  const listed = value.agents;
  if (!Array.isArray(listed) || listed.length === 0) return null;
  const agents: ChildAgent[] = [];
  for (const entry of listed) {
    if (!isRecord(entry)) return null;
    const { name, description } = entry;
    if (typeof name !== "string" || name.trim() === "") return null;
    if (typeof description !== "string") return null;
    if (agents.some((a) => a.name === name)) return null;
    agents.push({ name, description });
  }
  const max = value.maxOpen;
  if (typeof max !== "number" || !Number.isInteger(max) || max < 1 || max > MOST_OPEN) return null;
  return { agents, maxOpen: max };
}
