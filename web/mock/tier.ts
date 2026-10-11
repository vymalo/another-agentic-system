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
