"use client";

import { cn } from "@/lib/utils";
import { type AgentCommand, takesArguments } from "../lib/commands";

type Props = {
  id: string;
  optionId: (index: number) => string;
  options: readonly AgentCommand[];
  active: number;
  onPick: (command: AgentCommand) => void;
  onHover: (index: number) => void;
};

/**
 * The `/` menu of the agent's own commands (ADR 0079, `docs/api/commands-v1.md` section 6), above
 * the box, as the listbox of its combobox. A label says which list this is ("Agent commands": the
 * person's prompts open on the same trigger), then one row per command: `/name`, its `argumentHint`
 * beside it in the quiet type, and its description under it. The focus never leaves the textarea
 * (the keys and `aria-activedescendant` are the box's), so a press on a row does not take it.
 *
 * A name and a description are the agent's card's words: untrusted text, drawn as text and never as
 * an instruction. The listbox holds options and nothing else (ARIA's rule): the label is its sibling.
 */
export function CommandMenu({ id, optionId, options, active, onPick, onHover }: Props) {
  return (
    <div
      data-slot="command-menu"
      className="absolute inset-x-0 bottom-full z-20 mb-2 flex max-h-72 flex-col gap-0.5 overflow-y-auto rounded-2xl border bg-popover p-1.5 text-popover-foreground shadow-composer"
    >
      {/* which list this is: the agent's commands and the person's prompts share one trigger */}
      <p
        data-slot="command-menu-label"
        className="m-0 px-2.5 py-1 text-xs leading-4 text-muted-foreground"
      >
        Agent commands
      </p>
      <div id={id} role="listbox" aria-label="Agent commands" className="flex flex-col gap-0.5">
        {options.map((command, i) => (
          // biome-ignore lint/a11y/useKeyWithClickEvents: the keys are the textarea's (combobox pattern)
          <div
            key={command.name}
            id={optionId(i)}
            role="option"
            tabIndex={-1}
            aria-selected={i === active}
            data-slot="command-option"
            data-command={command.name}
            // a press must not take the focus from the box
            onMouseDown={(e) => e.preventDefault()}
            onMouseMove={() => i !== active && onHover(i)}
            onClick={() => onPick(command)}
            className={cn(
              "flex min-w-0 cursor-pointer flex-col gap-0.5 rounded-lg px-2.5 py-2",
              i === active && "bg-accent text-accent-foreground",
            )}
          >
            <span className="flex min-w-0 items-baseline gap-2">
              <span className="shrink-0 font-mono text-sm leading-5 font-medium">
                /{command.name}
              </span>
              {takesArguments(command) ? (
                <span className="truncate font-mono text-xs text-muted-foreground">
                  {command.argumentHint}
                </span>
              ) : null}
            </span>
            <span className="line-clamp-1 text-xs leading-4 text-muted-foreground">
              {command.description}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
