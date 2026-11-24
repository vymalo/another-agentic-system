"use client";

import { useAui, useAuiState } from "@assistant-ui/react";
import {
  type KeyboardEvent,
  type RefObject,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from "react";
import {
  type AgentCommand,
  commandQueryOf,
  insertionOf,
  matchingCommands,
} from "@/features/commands/lib/commands";
import { usePrompts } from "@/features/prompts/hooks/use-prompts";
import { matchingPrompts } from "@/features/prompts/lib/fill";
import type { ApiPromptSummary } from "@/lib/api/types";

/** One row of the one `/` menu: a command an agent serves, or a prompt the person owns or is given. */
export type SlashOption =
  | { kind: "command"; key: string; domId: string; command: AgentCommand }
  | { kind: "prompt"; key: string; domId: string; prompt: ApiPromptSummary };

type Args = {
  /** The commands the current agent serves; empty for an agent that serves none. */
  commands: readonly AgentCommand[];
  /** False while the box is not for a message (an agent waits for an answer): no "/" opens anything. */
  enabled: boolean;
  inputRef: RefObject<HTMLTextAreaElement | null>;
};

/**
 * The one `/` menu of the message box (ADR 0067 slice P3): the agent's own commands (ADR 0079,
 * `docs/api/commands-v1.md` section 6) and the person's prompts (docs/api/prompts-v1.md, "The
 * recipe") in **one list of two labelled groups**, commands first — the order of the epic's table —
 * for a message that is one `/word`. Arrow keys walk the two groups as one list, Enter or Tab picks
 * the active row, Escape closes the list until the query changes, and a click picks too.
 *
 * Picking keeps each path exactly as it was: a command inserts `/name` (with a space when it takes
 * arguments) and sends nothing, and a prompt reads itself, opens its fill form when it has questions,
 * and puts `renderPrompt`'s text in the box. **Nothing is ever sent** by a pick.
 *
 * `claims` is whether the box is one `/word` this menu answers for, open or dismissed: the mentions
 * menu must not open for it.
 */
export function useSlashMenu({ commands, enabled, inputRef }: Args) {
  const aui = useAui();
  const text = useAuiState((s) => s.composer.text);
  const trigger = useMemo(() => (enabled ? commandQueryOf(text) : null), [enabled, text]);
  const query = trigger?.query ?? "";

  const prompts = usePrompts({ enabled, listing: trigger !== null, inputRef });
  const promptList = prompts.state.status === "ready" ? prompts.state.prompts : [];

  const matchedCommands = useMemo(
    () => (trigger ? matchingCommands(commands, query) : []),
    [commands, query, trigger],
  );
  const matchedPrompts = useMemo(
    () => (trigger ? matchingPrompts(promptList, query) : []),
    [promptList, query, trigger],
  );

  const [dismissed, setDismissed] = useState<string | null>(null);
  const [cursor, setCursor] = useState<{ key: string; index: number }>({ key: "", index: 0 });
  const listboxId = useId();

  const options = useMemo<SlashOption[]>(
    () => [
      ...matchedCommands.map((command, i) => ({
        kind: "command" as const,
        key: `command-${command.name}`,
        domId: `${listboxId}-c-${i}`,
        command,
      })),
      ...matchedPrompts.map((prompt, i) => ({
        kind: "prompt" as const,
        key: `prompt-${prompt.id}`,
        domId: `${listboxId}-p-${i}`,
        prompt,
      })),
    ],
    [matchedCommands, listboxId, matchedPrompts],
  );

  // which halves the menu says something about: commands only when one matches, prompts whenever the
  // deployment offers a library (a line says that it is loading, empty or unreadable)
  const showCommands = matchedCommands.length > 0;
  const showPrompts = prompts.offered;
  // whether the menu holds a row at all: an empty listbox is invalid, so the line alone is a block
  const listed = options.length > 0;
  const claims = trigger !== null && (showCommands || showPrompts);
  const open = claims && dismissed !== query;
  const active = cursor.key === query ? Math.min(cursor.index, Math.max(options.length - 1, 0)) : 0;

  // the box stopped being one /word: the next one opens the list afresh (a command or a prompt that
  // was picked or dismissed is not remembered forever)
  useEffect(() => {
    if (!trigger) setDismissed(null);
  }, [trigger]);

  /** Puts `/name` in the box, the caret after it: the person types the arguments and sends it. */
  const pickCommand = useCallback(
    (command: AgentCommand) => {
      const next = insertionOf(command);
      aui.composer().setText(next);
      // a command that takes no arguments leaves a text that is still one /word: the list closes on the pick
      setDismissed(commandQueryOf(next)?.query ?? null);
      requestAnimationFrame(() => {
        const el = inputRef.current;
        el?.focus();
        el?.setSelectionRange(next.length, next.length);
      });
    },
    [aui, inputRef],
  );

  const pick = useCallback(
    (option: SlashOption | undefined) => {
      if (!option) return;
      if (option.kind === "command") pickCommand(option.command);
      else void prompts.pick(option.prompt, query);
    },
    [pickCommand, prompts, query],
  );

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!open || e.nativeEvent.isComposing) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (options.length === 0) return;
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setCursor({ key: query, index: (active + step + options.length) % options.length });
      return;
    }
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
    if ((e.key === "Enter" && !e.shiftKey && plain) || (e.key === "Tab" && !e.shiftKey && plain)) {
      if (options.length === 0) {
        // a half-typed command is not sent while the list is still being read; Tab may leave the box
        const reading = prompts.state.status === "loading" || prompts.state.status === "idle";
        if (e.key === "Enter" && reading) e.preventDefault();
        return;
      }
      // the pick, never a send: the box's Enter would post the half-typed query
      e.preventDefault();
      pick(options[active]);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setDismissed(query);
    }
  };

  const comboboxProps = !claims
    ? {}
    : open
      ? ({
          role: "combobox",
          "aria-autocomplete": "list",
          ...(listed ? { "aria-haspopup": "listbox" } : {}),
          "aria-expanded": true,
          "aria-controls": listboxId,
          ...(listed ? { "aria-activedescendant": options[active]?.domId } : {}),
        } as const)
      : ({
          "aria-autocomplete": "list",
          ...(listed ? { "aria-haspopup": "listbox" } : {}),
        } as const);

  return {
    /** The list is drawn: the box is one /word the menu answers for and was not dismissed. */
    open,
    /** The box is one /word this menu answers for, open or dismissed. */
    claims,
    query,
    listboxId,
    /** The commands group's rows, and the prompts group's, in the order they are drawn. */
    commandOptions: options.filter(
      (o): o is Extract<SlashOption, { kind: "command" }> => o.kind === "command",
    ),
    promptOptions: options.filter(
      (o): o is Extract<SlashOption, { kind: "prompt" }> => o.kind === "prompt",
    ),
    showPrompts,
    /** Whether the menu holds a row: false draws the prompts' line alone, not a listbox. */
    listed,
    /** The index of the active row in the flat list (the commands group first). */
    active,
    onKeyDown,
    comboboxProps,
    pick,
    /** Mouse over a row makes it the active one. */
    hover: (index: number) => setCursor({ key: query, index }),
    /** What the prompts half says while it loads, is empty, or could not be read. */
    prompts: {
      state: prompts.state,
      notice: prompts.notice,
      filling: prompts.filling,
      fill: prompts.fill,
      loadCurrent: prompts.loadCurrent,
      filled: prompts.filled,
      close: prompts.close,
    },
  };
}

export type SlashMenuView = ReturnType<typeof useSlashMenu>;
