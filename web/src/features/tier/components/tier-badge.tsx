"use client";

import { useAuiState } from "@assistant-ui/react";
import { createContext, type ReactNode, useContext, useSyncExternalStore } from "react";
import { Hint } from "@/components/hint";
import { cn } from "@/lib/utils";
import type { SentTiers } from "../lib/sent";
import { parseTier, TIER_INFO, type Tier } from "../lib/tier";
import { TIER_ICON } from "./tier-chip";

const Context = createContext<SentTiers | null>(null);

/** What a bubble needs to say the tier of a message this page sent (by message id). */
export function TiersProvider({ store, children }: { store: SentTiers; children: ReactNode }) {
  return <Context.Provider value={store}>{children}</Context.Provider>;
}

const NO_SUBSCRIBE = () => () => {};
const ZERO = () => 0;

/**
 * The tier of the user message being drawn: the one the log carried (`metadata.custom.tier`, a reload or
 * another tab), else the one this page sent it under (the stream does not echo a message to the run that
 * sent it). Anything but one of the four words is no tier.
 */
export function useMessageTier(): Tier | undefined {
  const store = useContext(Context);
  const id = useAuiState((s) => s.message.id);
  const custom = useAuiState((s) => s.message.metadata?.custom?.tier);
  useSyncExternalStore(
    store?.subscribe ?? NO_SUBSCRIBE,
    store?.getVersion ?? ZERO,
    store?.getVersion ?? ZERO,
  );
  return parseTier(custom) ?? store?.of(id);
}

/** A small badge with the tier a tier-aware agent worked under for a message; nothing for a message that named none. */
export function TierBadge({ tier }: { tier: Tier }) {
  const info = TIER_INFO[tier];
  const Icon = TIER_ICON[tier];
  const warns = tier === "bypass";
  return (
    <Hint label={`${info.label}: ${info.description}`} side="bottom">
      <span
        data-slot="tier-badge"
        data-tier={tier}
        className={cn(
          "mt-1 inline-flex h-5 items-center gap-1 self-end rounded-full px-1.5 text-xs leading-4",
          warns ? "bg-warning-soft font-medium text-warning" : "bg-muted text-muted-foreground",
        )}
      >
        <Icon aria-hidden="true" className="size-3" />
        <span>
          <span className="sr-only">Sent under the access tier </span>
          {info.label}
        </span>
      </span>
    </Hint>
  );
}

/** The badge of the message being drawn, when it was sent under a tier. */
export function MessageTierBadge() {
  const tier = useMessageTier();
  return tier === undefined ? null : <TierBadge tier={tier} />;
}
