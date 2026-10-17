import { useCallback, useEffect, useMemo, useState } from "react";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { readPick, writePick } from "../lib/pick";
import {
  ACCESS_TIER_URI,
  offeredTiers,
  parseOffer,
  type Shown,
  shownTier,
  type Tier,
  type TierOffer,
} from "../lib/tier";

/** What the composer needs of the access tier (ADR 0061): the chip, its notice and what goes with the next message. */
export type TierView = {
  /** What the agent's live card offers; null while it lists nothing usable: no chip is drawn. */
  offer: TierOffer | null;
  /** The tiers the person may pick: the agent's, up to their role's cap. */
  offered: readonly Tier[];
  /** What the chip shows, or null with no offer. */
  shown: Shown | null;
  /**
   * The tier sent with the next message: the person's pick while the card offers it, and while the card
   * cannot be read (the orchestrator reads it and decides, and refuses what it cannot check), else nothing.
   */
  send: Tier | undefined;
  /** The person picks a tier: it is sent with every message of the thread from now on. */
  pick: (tier: Tier) => void;
  /** A tier that was remembered and is no longer offered (the agent changed its tiers, or the cap did): forgotten, said once. */
  forgotten: Tier | null;
  /** Dismisses what `forgotten` said. */
  dismiss: () => void;
  /** The send that creates a thread was accepted: the pick of the new chat is the new thread's. */
  created: (threadId: string) => void;
};

/**
 * The access tier of the composer (ADR 0061, docs/api/access-tier-v1.md section 8).
 *
 * The chip exists only while the selected agent's live card lists the extension with well-formed
 * params (`capabilities`, read live and never cached: a card that cannot be read offers nothing, and
 * nothing is sent). Until the person picks, the chip shows the agent's `default` and **no tier is
 * sent**; a pick is sent with every message of the thread, remembered in the browser per thread
 * (a new chat keeps it in memory until the run that creates the thread is accepted: `created`), and
 * forgotten, with a word about it, once the agent no longer honours it or the person's cap no longer
 * allows it. The cap is the one **for this agent**, which the capabilities document carries beside its
 * offer (`maxTier`): the cap of `GET /api/me` spans every agent and can be higher.
 */
export function useTier({
  threadId,
  capabilities,
}: {
  threadId: string | null;
  capabilities: AgentCapabilities;
}): TierView {
  const [pick, setPick] = useState<Tier | undefined>(() =>
    threadId === null ? undefined : readPick(threadId),
  );
  const [forgotten, setForgotten] = useState<Tier | null>(null);

  const entry = capabilities.status === "ready" ? capabilities.entry?.(ACCESS_TIER_URI) : undefined;
  const offer = useMemo(() => (entry === undefined ? null : parseOffer(entry)), [entry]);
  const offered = useMemo(() => (offer ? offeredTiers(offer) : []), [offer]);

  // A pick the card or the cap no longer allows is gone, and said. Only once the card has been read:
  // a card not read yet (or not readable) says nothing, and a remembered pick waits for it.
  const read = capabilities.status === "ready";
  useEffect(() => {
    if (!read || pick === undefined) return;
    if (offer === null || !offered.includes(pick)) {
      setPick(undefined);
      if (threadId !== null) writePick(threadId, undefined);
      // an agent that lists no tier at all has no chip to say it
      if (offer !== null) setForgotten(pick);
    }
  }, [read, offer, offered, pick, threadId]);

  const choose = useCallback(
    (tier: Tier) => {
      setPick(tier);
      setForgotten(null);
      if (threadId !== null) writePick(threadId, tier);
    },
    [threadId],
  );
  const dismiss = useCallback(() => setForgotten(null), []);
  const created = useCallback(
    (id: string) => {
      if (pick !== undefined) writePick(id, pick);
    },
    [pick],
  );

  // A card not read (yet) or not readable does not unsay the pick: the message goes with it, and the
  // orchestrator, which reads the card itself, sends it or refuses it. Dropping it would run the agent
  // under its default, above what the person chose.
  const send =
    pick === undefined ? undefined : read ? (offered.includes(pick) ? pick : undefined) : pick;
  const shown = offer ? shownTier(offer, offered, pick) : null;
  return { offer, offered, shown, send, pick: choose, forgotten, dismiss, created };
}
