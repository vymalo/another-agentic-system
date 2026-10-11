/**
 * `access-tier/v1` as the mock reads it (docs/api/access-tier-v1.md, ADR 0061), written from the
 * contract and not from the web's own parser (`src/features/tier`), so that the two are two readings of
 * one document: the web's tests and the contract tests do not agree by sharing code.
 */

/** The URI an agent lists on its card, and the key of the capabilities document's `custom` under it. */
export const TIER_URI = "https://agents.vymalo.com/a2a/extensions/access-tier/v1";

/** `forwardedProps` key of a run that carries a message: one of the four words. */
export const TIER_PROP = "vymalo.tier";

/** The four tiers, lowest first. */
export const TIERS = ["plan", "edits", "auto", "bypass"] as const;
export type Tier = (typeof TIERS)[number];

const isTier = (v: unknown): v is Tier =>
  typeof v === "string" && (TIERS as readonly string[]).includes(v);

/** `params` of a card's extension: `tiers` (non-empty, each of the four once) and a `default` among them. */
export type Offer = { tiers: Tier[]; default: Tier };

/** A card that lists the extension badly does not list it: anything but well-formed params is no offer. */
export function readOffer(params: unknown): Offer | null {
  if (typeof params !== "object" || params === null || Array.isArray(params)) return null;
  const { tiers, default: preset } = params as Record<string, unknown>;
  if (!Array.isArray(tiers) || tiers.length === 0) return null;
  const seen = new Set<Tier>();
  for (const tier of tiers) {
    if (!isTier(tier) || seen.has(tier)) return null;
    seen.add(tier);
  }
  if (!isTier(preset) || !seen.has(preset)) return null;
  return { tiers: TIERS.filter((t) => seen.has(t)), default: preset };
}

/**
 * `forwardedProps["vymalo.tier"]`: one of the four words spelled exactly, or `null` or absent for none.
 * Anything else is a 400 whose detail names the key and the words.
 */
export function readTierProp(value: unknown): { tier: Tier | undefined } | { detail: string } {
  if (value === undefined || value === null) return { tier: undefined };
  if (isTier(value)) return { tier: value };
  return { detail: `${TIER_PROP} must be one of ${TIERS.join(", ")}` };
}

/**
 * What a session's test set for the card of an agent (`POST /__mock/tiers`): the raw `params` of the
 * extension on its card (taken as given, so a malformed one can be tried), or `"unreadable"` for a
 * card that cannot be read (the capabilities document lists nothing, and a message that names a tier
 * is a 503: set it after the chip is drawn to see a card that is gone when the message is sent).
 */
export type CardTiers = unknown | "unreadable";

/** What the live card of an agent says about `tier`, when a message names it. */
export function judgeTier(
  card: CardTiers | undefined,
  tier: Tier,
): "honoured" | "not-honoured" | "unreadable" {
  if (card === "unreadable") return "unreadable";
  const offer = readOffer(card);
  return offer?.tiers.includes(tier) ? "honoured" : "not-honoured";
}

const rank = (tier: Tier): number => TIERS.indexOf(tier);

/** The lower of two tiers; `b` is the cap, absent when there is none. */
const lowerOf = (a: Tier, b: Tier | undefined): Tier =>
  b !== undefined && rank(b) < rank(a) ? b : a;

/** The highest tier of `offer` that is not above `limit`; undefined when there is none. */
const highestWithin = (offer: Offer, limit: Tier): Tier | undefined =>
  offer.tiers.filter((t) => rank(t) <= rank(limit)).at(-1);

/** What decides the tier a message is written and sent under (docs/api/access-tier-v1.md section 7). */
export type Resolution =
  | { tier: Tier | undefined; read: boolean }
  | { status: 422 | 503; detail: string };

/**
 * The one place the mock decides the tier of a message, from the contract and not from the web:
 * `named` is what the person's message names, `inherited` the tier of the job it continues (an
 * answer to a blocked job) or of the message an edit replaces, `cap` the person's `maxTier` and
 * `card` what the agent's live card says.
 *
 * - A named tier is checked: the cap (422), then the card (422, or 503 when it cannot be read).
 * - With nothing named, nothing inherited and no cap there is no tier and no card is read.
 * - An inherited tier is lowered to the cap and to the highest tier the card honours below it; a
 *   card that lists none (or no extension) is a 422 that says to start a new chat.
 * - With a cap and nothing else, the highest tier the card offers that is not above the lower of its
 *   `default` and the cap (422 when there is none); a card without the extension gets none.
 */
export function resolveTier(input: {
  agentId: string;
  cap: Tier | undefined;
  card: CardTiers | undefined;
  named: Tier | undefined;
  inherited: Tier | undefined;
}): Resolution {
  const { agentId, cap, card, named, inherited } = input;
  if (named !== undefined && cap !== undefined && rank(named) > rank(cap)) {
    return { status: 422, detail: `the tier '${named}' is above what your role allows` };
  }
  if (named === undefined && inherited === undefined && cap === undefined) {
    return { tier: undefined, read: false };
  }
  if (card === "unreadable") {
    return { status: 503, detail: `the card of the agent ${agentId} could not be read` };
  }
  const offer = readOffer(card);
  if (named !== undefined) {
    return offer?.tiers.includes(named)
      ? { tier: named, read: true }
      : { status: 422, detail: `this agent does not take the tier '${named}'` };
  }
  if (inherited !== undefined) {
    if (!offer) {
      return {
        status: 422,
        detail: `this agent no longer lists access tiers, so the tier '${inherited}' of this chat cannot be kept: start a new chat`,
      };
    }
    const limit = lowerOf(inherited, cap);
    const kept = highestWithin(offer, limit);
    return kept
      ? { tier: kept, read: true }
      : {
          status: 422,
          detail: `this agent has no tier at or below '${limit}' any more, so the tier '${inherited}' of this chat cannot be kept: start a new chat`,
        };
  }
  if (!offer) return { tier: undefined, read: true };
  const given = highestWithin(offer, lowerOf(offer.default, cap));
  return given
    ? { tier: given, read: true }
    : { status: 422, detail: "this agent has no tier your role allows" };
}
