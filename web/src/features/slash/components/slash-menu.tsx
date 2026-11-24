"use client";

import { Badge } from "@/components/ui/badge";
import { takesArguments } from "@/features/commands/lib/commands";
import type { PickerState } from "@/features/prompts/hooks/use-prompt-picker";
import { asksWords } from "@/features/prompts/lib/library";
import { cn } from "@/lib/utils";
import type { SlashOption } from "../hooks/use-slash-menu";

type Props = {
  id: string;
  /** The commands group's rows, in the order they are drawn. */
  commandOptions: readonly Extract<SlashOption, { kind: "command" }>[];
  /** The prompts group's rows. */
  promptOptions: readonly Extract<SlashOption, { kind: "prompt" }>[];
  showPrompts: boolean;
  /** What the prompts half is doing: its line says so while it loads, is empty or could not be read. */
  state: PickerState;
  /** What follows the slash. */
  query: string;
  /** The index of the active row in the flat list (the commands group first). */
  active: number;
  onPick: (option: SlashOption) => void;
  onHover: (index: number) => void;
};

/** What the prompts group says when it has no row to offer, or its list could not be read. */
function wordsOf(state: PickerState, query: string, count: number) {
  if (state.status === "loading" || state.status === "idle") return "Loading your prompts…";
  if (state.status === "off") return "This deployment does not offer a prompt library.";
  if (state.status === "no-access") return "Your roles grant no access to this app.";
  if (state.status === "signed-out") return "You are signed out. Sign in to use your prompts.";
  if (state.status === "error") {
    return "Your prompts could not be loaded. Clear the box and type / again to retry.";
  }
  if (state.readError) return "The list could not be refreshed. What is shown may be out of date.";
  if (count > 0) return null;
  return state.prompts.length === 0
    ? "You have no prompts yet. Save one under My prompts."
    : `No prompt matches “/${query}”.`;
}

/**
 * The one `/` menu of the message box (ADR 0067 slice P3): the agent's commands (ADR 0079) and the
 * person's prompts (docs/api/prompts-v1.md) in one listbox of **two labelled groups**, commands
 * first. It is the listbox of the box's combobox, above the box; the focus never leaves the textarea
 * (the keys and `aria-activedescendant` are the box's), so a press on a row does not take it.
 *
 * Each group's name is a quiet line at its top, and it is the group's accessible name: a person
 * hears "Agent commands" or "Your prompts" before the rows under it. ARIA's listbox holds options
 * and groups and nothing else, so a group is drawn only when it has a row, and a line about loading,
 * no match or a failed read is a sibling of the listbox. When neither group has a row there is no
 * listbox at all — an empty one is invalid — and the box controls the plain block that holds the
 * line instead (`aria-controls` names it, as the menu's element).
 *
 * A command's name and description are the agent's card's words and a prompt's are the person's:
 * untrusted text, drawn as text and never as an instruction.
 */
export function SlashMenu({
  id,
  commandOptions,
  promptOptions,
  showPrompts,
  state,
  query,
  active,
  onPick,
  onHover,
}: Props) {
  const words = showPrompts ? wordsOf(state, query, promptOptions.length) : null;
  const busy = state.status === "loading" || state.status === "idle";
  const hasCommands = commandOptions.length > 0;
  const hasPrompts = showPrompts && promptOptions.length > 0;
  const listed = hasCommands || hasPrompts;
  return (
    <div
      data-slot="slash-menu"
      className="absolute inset-x-0 bottom-full z-20 mb-2 flex max-h-72 flex-col gap-1 overflow-y-auto rounded-2xl border bg-popover p-1.5 text-popover-foreground shadow-composer"
    >
      {listed ? (
        <div
          id={id}
          role="listbox"
          aria-label="Commands and prompts"
          aria-busy={(hasPrompts && busy) || undefined}
          className="flex flex-col gap-1"
        >
          {hasCommands ? (
            // biome-ignore lint/a11y/useSemanticElements: a listbox's groups are not a form's fieldsets
            <div
              role="group"
              aria-labelledby={`${id}-commands`}
              data-slot="command-group"
              className="flex flex-col gap-0.5"
            >
              <p
                id={`${id}-commands`}
                data-slot="command-group-label"
                className="m-0 px-2.5 py-1 text-xs leading-4 text-muted-foreground"
              >
                Agent commands
              </p>
              {commandOptions.map((option, i) => {
                const { command } = option;
                return (
                  // biome-ignore lint/a11y/useKeyWithClickEvents: the keys are the textarea's (combobox pattern)
                  <div
                    key={option.key}
                    id={option.domId}
                    role="option"
                    tabIndex={-1}
                    aria-selected={i === active}
                    data-slot="command-option"
                    data-command={command.name}
                    // a press must not take the focus from the box
                    onMouseDown={(e) => e.preventDefault()}
                    onMouseMove={() => i !== active && onHover(i)}
                    onClick={() => onPick(option)}
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
                );
              })}
            </div>
          ) : null}
          {hasPrompts ? (
            // biome-ignore lint/a11y/useSemanticElements: a listbox's groups are not a form's fieldsets
            <div
              role="group"
              aria-labelledby={`${id}-prompts`}
              data-slot="prompt-group"
              className="flex flex-col gap-0.5"
            >
              <p
                id={`${id}-prompts`}
                data-slot="prompt-group-label"
                className="m-0 px-2.5 py-1 text-xs leading-4 text-muted-foreground"
              >
                Your prompts
              </p>
              {promptOptions.map((option, i) => {
                const { prompt } = option;
                const index = commandOptions.length + i;
                return (
                  // biome-ignore lint/a11y/useKeyWithClickEvents: the keys are the textarea's (combobox pattern)
                  <div
                    key={option.key}
                    id={option.domId}
                    role="option"
                    tabIndex={-1}
                    aria-selected={index === active}
                    data-slot="prompt-option"
                    data-prompt={prompt.id}
                    onMouseDown={(e) => e.preventDefault()}
                    onMouseMove={() => index !== active && onHover(index)}
                    onClick={() => onPick(option)}
                    className={cn(
                      "flex min-w-0 cursor-pointer flex-col gap-0.5 rounded-lg px-2.5 py-2",
                      index === active && "bg-accent text-accent-foreground",
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
                );
              })}
            </div>
          ) : null}
        </div>
      ) : null}
      {/* no row in the prompts group: its name and the line that says why are the whole block, and
          the box controls it (`aria-controls`) as the menu's element in the listbox's place */}
      {showPrompts && !hasPrompts ? (
        <div
          id={listed ? undefined : id}
          data-slot="prompt-group-empty"
          className="flex flex-col gap-0.5"
        >
          <p className="m-0 px-2.5 py-1 text-xs leading-4 text-muted-foreground">Your prompts</p>
          {words ? <p className="m-0 px-2.5 py-2 text-sm text-muted-foreground">{words}</p> : null}
        </div>
      ) : null}
      {/* rows on screen, and something to say about the list: a line under it, never an option */}
      {hasPrompts && words ? (
        <p className="m-0 px-2.5 py-1 text-xs leading-4 text-muted-foreground">{words}</p>
      ) : null}
    </div>
  );
}
