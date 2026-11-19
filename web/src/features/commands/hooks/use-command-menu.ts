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
import { type AgentCommand, commandQueryOf, insertionOf, matchingCommands } from "../lib/commands";

type Args = {
  /** The commands the current agent serves; empty for an agent that serves none. */
  commands: readonly AgentCommand[];
  /** False while the box is not for a message (an agent waits for an answer): no "/" opens anything. */
  enabled: boolean;
  inputRef: RefObject<HTMLTextAreaElement | null>;
};

/**
 * The `/` menu of the agent's own commands (ADR 0079, `docs/api/commands-v1.md` section 6), in the
 * message box: a message that is one `/word` which **starts the name of a command the current agent
 * serves** opens the list above the box; Enter or Tab inserts the text (`/name` and, when the command
 * takes arguments, a space after it) and Escape closes the list until the query changes, and a click
 * inserts too. **Nothing is sent and nothing is added to the message**: the inserted text is the
 * message, and the text form is what invokes the command.
 *
 * `claims` says whether the typed text is one such query, whether the list is open or was dismissed:
 * the person's prompt library must not open for it (the two lists never open together), and Escape
 * then closes the menu rather than handing the same word to the other one.
 */
export function useCommandMenu({ commands, enabled, inputRef }: Args) {
  const aui = useAui();
  const text = useAuiState((s) => s.composer.text);
  const trigger = useMemo(
    () => (enabled && commands.length > 0 ? commandQueryOf(text) : null),
    [enabled, commands.length, text],
  );
  const options = useMemo(
    () => (trigger ? matchingCommands(commands, trigger.query) : []),
    [trigger, commands],
  );

  const [dismissed, setDismissed] = useState<string | null>(null);
  const [cursor, setCursor] = useState<{ key: string; index: number }>({ key: "", index: 0 });
  const listboxId = useId();

  const key = trigger?.query ?? "";
  const claims = options.length > 0;
  const open = claims && dismissed !== key;
  const active = cursor.key === key ? Math.min(cursor.index, Math.max(options.length - 1, 0)) : 0;
  const optionId = useCallback((i: number) => `${listboxId}-${i}`, [listboxId]);

  // the box stopped being one /word: the next one opens the list afresh (the same command can be
  // used again, and one that was dismissed is not remembered forever)
  useEffect(() => {
    if (!trigger) setDismissed(null);
  }, [trigger]);

  /** Puts `/name` in the box, the caret after it: the person types the arguments and sends it. */
  const pick = useCallback(
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

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!open || e.nativeEvent.isComposing) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setCursor({ key, index: (active + step + options.length) % options.length });
      return;
    }
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
    if ((e.key === "Enter" && !e.shiftKey && plain) || (e.key === "Tab" && !e.shiftKey && plain)) {
      const command = options[active];
      if (!command) return;
      // the pick, never a send: the box's Enter would post the half-typed query
      e.preventDefault();
      pick(command);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setDismissed(key);
    }
  };

  const comboboxProps =
    enabled && commands.length > 0
      ? open
        ? ({
            role: "combobox",
            "aria-autocomplete": "list",
            "aria-haspopup": "listbox",
            "aria-expanded": true,
            "aria-controls": listboxId,
            ...(options.length > 0 ? { "aria-activedescendant": optionId(active) } : {}),
          } as const)
        : ({ "aria-autocomplete": "list", "aria-haspopup": "listbox" } as const)
      : {};

  return {
    /** The list is drawn: the text is a query that matches a command and was not dismissed. */
    open,
    /** The text is a query that matches a command of this agent, open or dismissed. */
    claims,
    query: key,
    options,
    active,
    listboxId,
    optionId,
    onKeyDown,
    comboboxProps,
    pick,
    /** Mouse over an option makes it the active one. */
    hover: (index: number) => setCursor({ key, index }),
  };
}

export type CommandMenuView = ReturnType<typeof useCommandMenu>;
