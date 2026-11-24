"use client";

import { BrainIcon, ChevronDownIcon, CpuIcon, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { Hint } from "@/components/hint";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { PickView } from "../hooks/use-pick";
import type { Effort, ModelChoice, PickListing, ShownPick } from "../lib/pick";

/** The words each reasoning level shows. */
export const EFFORT_LABEL: Record<Effort, string> = {
  none: "None",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
};

/**
 * The menu value of the explicit agent-default entry. It carries a space, so it can never be a
 * model alias (the contract's alphabet is `a-z0-9._-`) and never collides with a choice.
 */
const AGENT_DEFAULT = "agent default";

const CHIP =
  "inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-full px-2.5 text-[0.8125rem] text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50";

type Props = {
  view: PickView;
  /** Reads the agent list again, live cards and all: opening a menu reads them again. */
  refresh?: () => void;
  /** Who answers under the pick ("Adam"), for the words of the menu. */
  agentName: string;
};

/**
 * The model and the reasoning effort chips of the composer (ADR 0086, `model-pick/v1` and adam's
 * `reasoning/v1`, [`docs/api/model-pick-v1.md`](../../../../../docs/api/model-pick-v1.md)): one
 * chip per axis, beside the access tier.
 *
 * A chip is drawn **only** while the selected agent's live card lists that axis (the listing is the
 * signal): no control, no disabled button and no placeholder otherwise. Until the person picks it
 * shows what the message runs under without a pick, the agent's own default, and says so
 * ("default"); a card that names no default shows the explicit "Agent default" entry instead. The
 * menu's first entry is always "Agent default": choosing it sends nothing on that axis, exactly
 * what no pick means, and lets a person go back from a choice. A pick is sent with every message
 * of the thread from then on. The listing is never a check: the orchestrator reads the card itself
 * and refuses a pick it does not offer. Radix's menu is the keyboard contract: arrows move, Enter
 * picks, Escape closes and the focus goes back to the button.
 */
export function PickChips({ view, refresh, agentName }: Props) {
  const { shown, listing } = view;
  if (!shown || !listing) return null;
  return (
    <>
      {shown.model ? (
        <ModelChip
          view={view}
          refresh={refresh}
          agentName={agentName}
          listing={listing}
          shown={shown.model}
        />
      ) : null}
      {shown.effort ? (
        <EffortChip
          view={view}
          refresh={refresh}
          agentName={agentName}
          listing={listing}
          shown={shown.effort}
        />
      ) : null}
    </>
  );
}

type ModelProps = Props & {
  listing: PickListing;
  shown: NonNullable<ShownPick["model"]>;
};

function ModelChip({ view, refresh, agentName, listing, shown }: ModelProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const named = shown.id !== undefined;
  const label = named ? shown.label || shown.id : "Agent default";
  const name = `Model: ${label}${named && !shown.picked ? " (the agent's default)" : ""}`;
  return (
    <DropdownMenu
      modal={false}
      onOpenChange={(open) => {
        setMenuOpen(open);
        // the agent's card is read again each time the menu opens: it is never cached
        if (open) refresh?.();
      }}
    >
      <Hint
        label={
          <span>
            {label}: the model {agentName} answers with
            {shown.picked ? "." : ", unless you pick another."}
          </span>
        }
        side="top"
        suppressed={menuOpen}
      >
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={name}
            data-slot="pick-chip"
            data-axis="model"
            data-picked={shown.picked}
            className={CHIP}
          >
            <CpuIcon aria-hidden="true" className="size-4" />
            <span>{label}</span>
            {shown.picked || !named ? null : (
              <span data-slot="pick-default" className="text-xs">
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
        <DropdownMenuLabel aria-hidden="true">The model {agentName} answers with</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={shown.picked ? (shown.id ?? AGENT_DEFAULT) : AGENT_DEFAULT}
          onValueChange={(value) => {
            if (value === AGENT_DEFAULT) view.pick({ model: undefined });
            else if (value.length > 0) view.pick({ model: value });
          }}
        >
          <AgentDefaultItem icon={CpuIcon} named={named ? label : undefined} />
          {listing.models.map((model) => (
            <ModelItem key={model.id} model={model} isDefault={model.id === listing.modelDefault} />
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <p data-slot="pick-hint" className="px-2.5 py-1.5 text-xs leading-4 text-muted-foreground">
          {shown.picked
            ? "Sent with each message of this chat."
            : named
              ? `Until you pick one, ${agentName} answers with ${label}, its default.`
              : `Until you pick one, ${agentName} chooses the model.`}
        </p>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function AgentDefaultItem({ icon: Icon, named }: { icon: LucideIcon; named?: string }) {
  return (
    <DropdownMenuRadioItem
      value={AGENT_DEFAULT}
      textValue="Agent default"
      data-slot="pick-item"
      data-agent-default="true"
    >
      <Icon aria-hidden="true" className="mt-0.5 size-5 shrink-0 text-muted-foreground" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="text-sm leading-5 font-medium">
          Agent default
          <span className="ms-1.5 text-xs font-normal text-muted-foreground">
            {named ?? "the agent chooses"}
          </span>
        </span>
      </span>
    </DropdownMenuRadioItem>
  );
}

function ModelItem({ model, isDefault }: { model: ModelChoice; isDefault: boolean }) {
  return (
    <DropdownMenuRadioItem
      value={model.id}
      textValue={model.label || model.id}
      data-slot="pick-item"
      data-model={model.id}
    >
      <CpuIcon aria-hidden="true" className="mt-0.5 size-5 shrink-0 text-muted-foreground" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="text-sm leading-5 font-medium">
          {model.label || model.id}
          {isDefault ? (
            <span className="ms-1.5 text-xs font-normal text-muted-foreground">default</span>
          ) : null}
        </span>
      </span>
    </DropdownMenuRadioItem>
  );
}

type EffortProps = Props & {
  listing: PickListing;
  shown: NonNullable<ShownPick["effort"]>;
};

function EffortChip({ view, refresh, agentName, listing, shown }: EffortProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const level = shown.level;
  const named = level !== undefined;
  const label = level !== undefined ? EFFORT_LABEL[level] : "Agent default";
  const name = `Reasoning effort: ${label}${
    named && !shown.picked ? " (the agent's default)" : ""
  }`;
  return (
    <DropdownMenu
      modal={false}
      onOpenChange={(open) => {
        setMenuOpen(open);
        // the agent's card is read again each time the menu opens: it is never cached
        if (open) refresh?.();
      }}
    >
      <Hint
        label={
          <span>
            {label}: how hard {agentName} thinks
            {shown.picked ? "." : ", unless you pick another level."}
          </span>
        }
        side="top"
        suppressed={menuOpen}
      >
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={name}
            data-slot="pick-chip"
            data-axis="effort"
            data-picked={shown.picked}
            className={CHIP}
          >
            <BrainIcon aria-hidden="true" className="size-4" />
            <span>{label}</span>
            {shown.picked || !named ? null : (
              <span data-slot="pick-default" className="text-xs">
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
        <DropdownMenuLabel aria-hidden="true">How hard {agentName} thinks</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={shown.picked ? (shown.level ?? AGENT_DEFAULT) : AGENT_DEFAULT}
          onValueChange={(value) => {
            if (value === AGENT_DEFAULT) view.pick({ effort: undefined });
            else {
              const level = listing.efforts.find((effort) => effort === value);
              if (level !== undefined) view.pick({ effort: level });
            }
          }}
        >
          <AgentDefaultItem icon={BrainIcon} named={named ? label : undefined} />
          {listing.efforts.map((level) => (
            <EffortItem key={level} level={level} isDefault={level === listing.effortDefault} />
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <p data-slot="pick-hint" className="px-2.5 py-1.5 text-xs leading-4 text-muted-foreground">
          {shown.picked
            ? "Sent with each message of this chat."
            : named
              ? `Until you pick one, ${agentName} works at ${label}, its default.`
              : `Until you pick one, ${agentName} chooses how hard it thinks.`}
        </p>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function EffortItem({ level, isDefault }: { level: Effort; isDefault: boolean }) {
  const label = EFFORT_LABEL[level];
  return (
    <DropdownMenuRadioItem
      value={level}
      textValue={label}
      data-slot="pick-item"
      data-effort={level}
    >
      <BrainIcon aria-hidden="true" className="mt-0.5 size-5 shrink-0 text-muted-foreground" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="text-sm leading-5 font-medium">
          {label}
          {isDefault ? (
            <span className="ms-1.5 text-xs font-normal text-muted-foreground">default</span>
          ) : null}
        </span>
      </span>
    </DropdownMenuRadioItem>
  );
}
