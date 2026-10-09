"use client";

import { XIcon } from "lucide-react";
import { Hint } from "@/components/hint";
import { agentNamed } from "@/features/agents/lib/selection";
import type { ApiAgent } from "@/lib/api/types";
import type { Mention } from "../lib/mentions";

/**
 * The agents the message in the box mentions, as chips after the tools picker: a list named
 * "Mentioned agents", each chip the agent's name and a button that takes the mention off (its label
 * leaves the text too: the text and the references are one thing, and a chip that stayed after its
 * words had gone would say something the message does not). A mention whose label is edited away
 * has no chip: that is how the person sees it was dropped. Nothing when nobody is mentioned.
 */
export function MentionChips({
  mentions,
  agents,
  onRemove,
}: {
  mentions: readonly Mention[];
  agents: readonly ApiAgent[];
  onRemove: (mention: Mention) => void;
}) {
  if (mentions.length === 0) return null;
  return (
    <ul aria-label="Mentioned agents" className="flex min-w-0 flex-wrap items-center gap-1.5">
      {mentions.map((m) => {
        const name = agentNamed(agents, m.agentId)?.name ?? m.agentId;
        return (
          <li
            key={`${m.start}:${m.agentId}`}
            data-slot="mention-chip"
            data-agent={m.agentId}
            className="inline-flex h-7 max-w-full min-w-0 items-center gap-1 rounded-full bg-muted ps-2.5 pe-0.5 text-[0.8125rem] text-foreground"
          >
            <span className="min-w-0 truncate">{name}</span>
            <Hint label={`Remove the mention of ${name}`} side="top">
              <button
                type="button"
                aria-label={`Remove the mention of ${name}`}
                onClick={() => onRemove(m)}
                className="inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full text-muted-foreground outline-none hover:bg-background hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                <XIcon aria-hidden="true" className="size-3.5" />
              </button>
            </Hint>
          </li>
        );
      })}
    </ul>
  );
}
