"use client";

import {
  ChevronDownIcon,
  EyeIcon,
  type LucideIcon,
  PencilLineIcon,
  TriangleAlertIcon,
  ZapIcon,
} from "lucide-react";
import { useState } from "react";
import { Hint } from "@/components/hint";
import { InlineStatus } from "@/components/inline-status";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { cn } from "@/lib/utils";
import type { TierView } from "../hooks/use-tier";
import { parseTier, TIER_INFO, type Tier } from "../lib/tier";

/** The icon of each tier. Bypass is the one that warns, by its shape and its words as well as its colour. */
export const TIER_ICON: Record<Tier, LucideIcon> = {
  plan: EyeIcon,
  edits: PencilLineIcon,
  auto: ZapIcon,
  bypass: TriangleAlertIcon,
};

type Props = {
  view: TierView;
  /** The agent's card, read live: opening the menu reads it again. */
  capabilities: AgentCapabilities;
  /** Who works under the tier ("Adam"), for the words of the menu. */
  agentName: string;
};

/**
 * The access tier chip, in the composer's toolbar beside the tools picker (ADR 0061,
 * docs/api/access-tier-v1.md section 8): the tier the agent works under for the message, and a menu of
 * the tiers it honours, up to what the person's roles allow, each with its icon and one line.
 *
 * It is drawn **only** while the selected agent's card lists the extension: nothing otherwise, no
 * disabled control and no placeholder. Until the person picks it shows the agent's default and says
 * so ("default"), and sends no tier; a pick is sent with every message of the thread. Bypass, which
 * never asks, is marked by a warning icon and the warning colours, and its line says so in words.
 * Radix's menu is the keyboard contract: arrows move, Enter picks, Escape closes and the focus goes
 * back to the button.
 */
export function TierChip({ view, capabilities, agentName }: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const { shown, offer } = view;
  if (!shown || !offer) return null;
  const info = TIER_INFO[shown.tier];
  const Icon = TIER_ICON[shown.tier];
  const warns = shown.tier === "bypass";
  const name = `Access tier: ${info.label}${shown.picked ? "" : " (the agent's default)"}`;
  return (
    <DropdownMenu
      modal={false}
      onOpenChange={(open) => {
        setMenuOpen(open);
        // the agent's card is read again each time the menu opens: it is never cached
        if (open) capabilities.refresh();
      }}
    >
      <Hint
        label={
          <span>
            {info.label}: {info.description}
            {shown.picked ? "" : " (the default)"}
          </span>
        }
        side="top"
        suppressed={menuOpen}
      >
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={name}
            data-slot="tier-chip"
            data-tier={shown.tier}
            data-picked={shown.picked}
            className={cn(
              "inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-full px-2.5 text-[0.8125rem] outline-none focus-visible:ring-3 focus-visible:ring-ring/50",
              warns
                ? "bg-warning-soft font-medium text-warning hover:bg-warning-soft/80"
                : "text-muted-foreground hover:bg-muted hover:text-foreground",
            )}
          >
            <Icon aria-hidden="true" className="size-4" />
            <span>{info.label}</span>
            {shown.picked ? null : (
              <span data-slot="tier-default" className="text-xs">
                default
              </span>
            )}
            <ChevronDownIcon aria-hidden="true" className="size-3.5 opacity-70" />
          </button>
        </DropdownMenuTrigger>
      </Hint>
      <DropdownMenuContent
        align="start"
        side="top"
        className="max-h-(--radix-dropdown-menu-content-available-height) w-[min(21rem,calc(100vw-1rem))] overflow-y-auto p-1.5"
      >
        <DropdownMenuLabel aria-hidden="true">How much {agentName} may do alone</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={shown.picked ? shown.tier : ""}
          onValueChange={(value) => {
            const tier = parseTier(value);
            if (tier !== undefined) view.pick(tier);
          }}
        >
          {view.offered.map((tier) => (
            <TierItem key={tier} tier={tier} isDefault={tier === offer.default} />
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <p data-slot="tier-hint" className="px-2.5 py-1.5 text-xs leading-4 text-muted-foreground">
          {shown.picked
            ? "Sent with each message of this chat."
            : `Until you pick one, ${agentName} works under its default, ${TIER_INFO[offer.default].label}.`}
        </p>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function TierItem({ tier, isDefault }: { tier: Tier; isDefault: boolean }) {
  const info = TIER_INFO[tier];
  const Icon = TIER_ICON[tier];
  const warns = tier === "bypass";
  return (
    <DropdownMenuRadioItem
      value={tier}
      textValue={info.label}
      data-slot="tier-item"
      data-tier={tier}
    >
      <Icon
        aria-hidden="true"
        className={cn("mt-0.5 size-5 shrink-0", warns ? "text-warning" : "text-muted-foreground")}
      />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className={cn("text-sm leading-5 font-medium", warns && "text-warning")}>
          {info.label}
          {isDefault ? (
            <span className="ms-1.5 text-xs font-normal text-muted-foreground">default</span>
          ) : null}
        </span>
        <span className="text-xs leading-4 text-muted-foreground">{info.description}</span>
      </span>
    </DropdownMenuRadioItem>
  );
}

/**
 * Said under the composer when a tier the person had picked is not offered any more (the agent changed
 * the tiers it honours, or the person's cap went down): it was forgotten, and the agent's default applies.
 */
export function TierNotice({ view, agentName }: { view: TierView; agentName: string }) {
  const { forgotten, offer } = view;
  if (forgotten === null || offer === null) return null;
  return (
    <InlineStatus tone="warning" role="status" action={{ label: "Dismiss", onClick: view.dismiss }}>
      <span data-slot="tier-notice">
        Your earlier choice, {TIER_INFO[forgotten].label}, is not offered any more. {agentName}{" "}
        works under its default, {TIER_INFO[offer.default].label}.
      </span>
    </InlineStatus>
  );
}
