"use client";

import { InlineStatus } from "@/components/inline-status";
import {
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import type { AgentsView } from "@/features/agents/hooks/use-agents";

/** What the person is told when a source of agents could not be read (ADR 0022). */
export const REGISTRY_UNREACHABLE_TEXT =
  "The agent registry is unreachable; showing the configured agents only.";

/**
 * The line under the greeting of a new chat when the platform's registry could not be read: the
 * agents of the configuration are listed and the registry's are not, and the person should know the
 * list is not the whole of it. Nothing when every source answered.
 */
export function RegistryNotice({ agents }: { agents: AgentsView }) {
  if (agents.registry.unreachable.length === 0) return null;
  return (
    <InlineStatus tone="warning" role="status" action={{ label: "Retry", onClick: agents.retry }}>
      {REGISTRY_UNREACHABLE_TEXT}
    </InlineStatus>
  );
}

/**
 * The same in the picker's menu (the `notice` slot of `AgentMenu`): a line and a menu item to retry.
 * A menu may hold only items, groups and labels, so the line is a label (no live region, which a
 * menu does not allow): it is read with the menu, and the item after it says what it retries.
 */
export function RegistryMenuNotice({ agents }: { agents: AgentsView }) {
  if (agents.registry.unreachable.length === 0) return null;
  return (
    <>
      <DropdownMenuSeparator />
      <DropdownMenuLabel className="pt-1.5 font-normal whitespace-normal text-warning">
        {REGISTRY_UNREACHABLE_TEXT}
      </DropdownMenuLabel>
      <DropdownMenuItem onSelect={agents.retry}>Retry</DropdownMenuItem>
    </>
  );
}
