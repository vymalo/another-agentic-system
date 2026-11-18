"use client";

import { Badge } from "@/components/ui/badge";
import type { ApiPromptSummary } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import type { PickerState } from "../hooks/use-prompt-picker";
import { asksWords } from "../lib/library";

type Props = {
  id: string;
  optionId: (index: number) => string;
  state: PickerState;
  /** What follows the slash. */
  query: string;
  options: readonly ApiPromptSummary[];
  active: number;
  onPick: (prompt: ApiPromptSummary) => void;
  onHover: (index: number) => void;
};

/** What the menu says when it has no row to offer, or its list could not be read. */
function wordsOf(state: PickerState, query: string, options: readonly ApiPromptSummary[]) {
  if (state.status === "loading" || state.status === "idle") return "Loading your prompts…";
  if (state.status === "off") return "This deployment does not offer a prompt library.";
  if (state.status === "no-access") return "Your roles grant no access to this app.";
  if (state.status === "signed-out") return "You are signed out. Sign in to use your prompts.";
  if (state.status === "error") {
    return "Your prompts could not be loaded. Clear the box and type / again to retry.";
  }
  if (state.readError) return "The list could not be refreshed. What is shown may be out of date.";
  if (options.length > 0) return null;
  return state.prompts.length === 0
    ? "You have no prompts yet. Save one under My prompts."
    : `No prompt matches “/${query}”.`;
}

/**
 * The `/` menu of the message box: the prompts the person owns and the ones shared with them, above
 * the box, as the listbox of its combobox. The focus never leaves the textarea (the keys and
 * `aria-activedescendant` are the box's), so a press on a row does not take it. A prompt's words are
 * plain text, and a shared prompt names no author (the API gives none).
 *
 * The listbox holds options and nothing else (ARIA's rule): a line about loading, no match or a
 * failed read is its sibling, above it.
 */
export function PromptMenu({
  id,
  optionId,
  state,
  query,
  options,
  active,
  onPick,
  onHover,
}: Props) {
  const words = wordsOf(state, query, options);
  const busy = state.status === "loading" || state.status === "idle";
  return (
    <div
      data-slot="prompt-menu"
      className="absolute inset-x-0 bottom-full z-20 mb-2 flex max-h-72 flex-col gap-0.5 overflow-y-auto rounded-2xl border bg-popover p-1.5 text-popover-foreground shadow-composer"
    >
      {words ? <p className="m-0 px-2.5 py-2 text-sm text-muted-foreground">{words}</p> : null}
      <div
        id={id}
        role="listbox"
        aria-label="Prompts"
        aria-busy={busy || undefined}
        className="flex flex-col gap-0.5"
      >
        {options.map((prompt, i) => (
          // biome-ignore lint/a11y/useKeyWithClickEvents: the keys are the textarea's (combobox pattern)
          <div
            key={prompt.id}
            id={optionId(i)}
            role="option"
            tabIndex={-1}
            aria-selected={i === active}
            data-slot="prompt-option"
            data-prompt={prompt.id}
            // a press must not take the focus from the box
            onMouseDown={(e) => e.preventDefault()}
            onMouseMove={() => i !== active && onHover(i)}
            onClick={() => onPick(prompt)}
            className={cn(
              "flex min-w-0 cursor-pointer flex-col gap-0.5 rounded-lg px-2.5 py-2",
              i === active && "bg-accent text-accent-foreground",
            )}
          >
            <span className="flex min-w-0 items-baseline gap-2">
              <span className="shrink-0 font-mono text-xs text-muted-foreground">
                /{prompt.name}
              </span>
              <span className="truncate text-sm leading-5 font-medium">{prompt.title}</span>
              {prompt.own ? null : (
                <Badge variant="secondary" className="ms-auto shrink-0">
                  Shared
                </Badge>
              )}
            </span>
            <span className="line-clamp-1 text-xs leading-4 text-muted-foreground">
              {prompt.description || asksWords(prompt.variableCount)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
