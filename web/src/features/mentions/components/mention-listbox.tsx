"use client";

import type { ApiAgent } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { labelOf } from "../lib/mentions";

type Props = {
  id: string;
  optionId: (index: number) => string;
  options: readonly ApiAgent[];
  active: number;
  onPick: (agent: ApiAgent) => void;
  onHover: (index: number) => void;
};

/**
 * The agents a person may mention, above the box: the listbox of the composer's combobox. The
 * focus never leaves the textarea (`aria-activedescendant` names the active option there), so the
 * options are not tab stops and a press on one does not take the focus from the box. Each shows the
 * agent's name, the label that goes into the text and, when the deployment says what it is for, one
 * line of it.
 */
export function MentionListbox({ id, optionId, options, active, onPick, onHover }: Props) {
  return (
    <div
      id={id}
      role="listbox"
      aria-label="Agents to mention"
      data-slot="mention-list"
      className="absolute inset-x-0 bottom-full z-20 mb-2 flex max-h-64 flex-col gap-0.5 overflow-y-auto rounded-2xl border bg-popover p-1.5 text-popover-foreground shadow-composer"
    >
      {options.map((agent, i) => (
        // biome-ignore lint/a11y/useKeyWithClickEvents: the keys are the textarea's (combobox pattern)
        <div
          key={agent.id}
          id={optionId(i)}
          role="option"
          tabIndex={-1}
          aria-selected={i === active}
          data-slot="mention-option"
          data-agent={agent.id}
          // a press must not take the focus from the box
          onMouseDown={(e) => e.preventDefault()}
          onMouseMove={() => i !== active && onHover(i)}
          onClick={() => onPick(agent)}
          className={cn(
            "flex min-w-0 cursor-pointer flex-col gap-0.5 rounded-lg px-2.5 py-2",
            i === active && "bg-accent text-accent-foreground",
          )}
        >
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="truncate text-sm leading-5 font-medium">{agent.name}</span>
            <span className="shrink-0 text-xs leading-4 text-muted-foreground">
              {labelOf(agent.id)}
            </span>
          </span>
          {agent.description ? (
            <span className="line-clamp-1 text-xs leading-4 text-muted-foreground">
              {agent.description}
            </span>
          ) : null}
        </div>
      ))}
    </div>
  );
}
