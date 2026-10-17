/**
 * The access tier of a message (ADR 0061, docs/api/access-tier-v1.md): how much the agent may do on
 * its own for one message, one of four words. Pure, so the chip, the badge and the tests say the same
 * thing. The orchestrator enforces none of it (the agent does); it checks the shape, the person's role
 * cap and the agent's live card when the message is written.
 */

/** The A2A extension an agent lists when it works under the tier a person picks (docs/api/access-tier-v1.md). */
export const ACCESS_TIER_URI = "https://agents.vymalo.com/a2a/extensions/access-tier/v1";

/** `forwardedProps["vymalo.tier"]` of a run that carries a message: one of the four words. */
export const TIER_PROP = "vymalo.tier";

/** `metadata["vymalo.tier"]` of a user message's `TEXT_MESSAGE_START`: the tier the message was sent under. */
export const TIER_KEY = "vymalo.tier";

/** The four tiers, ordered by how much the agent does alone: later is more. */
export const TIERS = ["plan", "edits", "auto", "bypass"] as const;
export type Tier = (typeof TIERS)[number];

/** A word that is one of the four, spelled exactly; anything else is no tier. */
export const parseTier = (value: unknown): Tier | undefined =>
  typeof value === "string" ? TIERS.find((t) => t === value) : undefined;

/**
 * What the capabilities document says under the extension: the tiers the agent honours (lowest first),
 * its default, and `max`, the highest tier the person asking may use **of this agent** (the member
 * `maxTier`, over the roles that let them invoke it; undefined when no role of theirs caps it).
 */
export type TierOffer = { tiers: readonly Tier[]; default: Tier; max?: Tier };

/**
 * `custom[ACCESS_TIER_URI]` of the capabilities document, read as strictly as the contract has it:
 * `tiers` a non-empty list of the four words, each at most once, `default` one of them, and `maxTier`,
 * when there, one of the four. Anything else is no offer, so no chip: the orchestrator never puts a
 * malformed one there, and a client that got one fails closed.
 */
export function parseOffer(value: unknown): TierOffer | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const { tiers, default: preset, maxTier } = value as Record<string, unknown>;
  if (!Array.isArray(tiers) || tiers.length === 0) return null;
  const parsed: Tier[] = [];
  for (const word of tiers) {
    const tier = parseTier(word);
    if (tier === undefined || parsed.includes(tier)) return null;
    parsed.push(tier);
  }
  const def = parseTier(preset);
  if (def === undefined || !parsed.includes(def)) return null;
  const max = maxTier === undefined ? undefined : parseTier(maxTier);
  if (maxTier !== undefined && max === undefined) return null;
  return {
    tiers: TIERS.filter((t) => parsed.includes(t)),
    default: def,
    ...(max === undefined ? {} : { max }),
  };
}

/**
 * The tiers the person is offered: the ones the agent honours, up to the highest their roles allow
 * **for this agent** (`max` of the offer, absent when no role caps it; not `maxTier` of `GET /api/me`,
 * which spans every agent and can be higher). A hint: the orchestrator checks the cap when the
 * message is written.
 */
export const offeredTiers = (offer: TierOffer): Tier[] =>
  offer.tiers.filter(
    (t) => offer.max === undefined || TIERS.indexOf(t) <= TIERS.indexOf(offer.max),
  );

/** What a tier is called, how it is drawn and what it means in one line. */
export type TierInfo = {
  /** The name on the chip and in the menu. */
  label: string;
  /** One line: what the agent may do on its own under it. */
  description: string;
};

export const TIER_INFO: Record<Tier, TierInfo> = {
  plan: { label: "Plan", description: "Read and propose" },
  edits: { label: "Edits", description: "Change files, open PRs" },
  auto: { label: "Auto", description: "Merge and create too" },
  bypass: { label: "Bypass", description: "Anything, never asks" },
};

/**
 * What the chip shows. The person's pick when they made one and it is still offered; else the tier the
 * message runs under when none is sent (`defaultFor`). `picked` is what is sent with each message:
 * nothing until the person picks.
 */
export type Shown = {
  /** The tier on the chip. */
  tier: Tier;
  /** The person picked it (and it is sent); false: nothing is sent, and it is what runs without a pick. */
  picked: boolean;
  /**
   * Not picked, and below the agent's own default because the person's role allows no more: the
   * orchestrator runs their message under it (docs/api/access-tier-v1.md section 7).
   */
  capped: boolean;
};

/**
 * The tier a message runs under when the person picks none, as the orchestrator decides it: the agent's
 * `default` when the person's roles do not hold it back, else the highest tier the agent offers that
 * the person may use and that is not above the default. Nothing when the agent offers no tier the
 * person may use (the orchestrator refuses the message: 422). `offered` is `offeredTiers(offer)`.
 */
export function defaultFor(offer: TierOffer, offered: readonly Tier[]): Tier | undefined {
  const limit = TIERS.indexOf(offer.default);
  return offered.filter((t) => TIERS.indexOf(t) <= limit).at(-1);
}

/** What the chip shows, or null when the person may use no tier of this agent and has picked none. */
export function shownTier(
  offer: TierOffer,
  offered: readonly Tier[],
  pick: Tier | undefined,
): Shown | null {
  if (pick !== undefined && offered.includes(pick)) {
    return { tier: pick, picked: true, capped: false };
  }
  const resting = defaultFor(offer, offered);
  if (resting === undefined) return null;
  return { tier: resting, picked: false, capped: resting !== offer.default };
}
