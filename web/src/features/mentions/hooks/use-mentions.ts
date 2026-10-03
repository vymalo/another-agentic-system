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
  useSyncExternalStore,
} from "react";
import type { ApiAgent } from "@/lib/api/types";
import {
  insertMention,
  MAX_MENTIONS,
  type Mention,
  matching,
  mentionable,
  reconcile,
  triggerAt,
  withoutMention,
} from "../lib/mentions";
import type { MentionsStore } from "../lib/store";

type Args = {
  /** The agents the person may mention here: invokable, and not the thread's own agent. */
  agents: readonly ApiAgent[];
  store: MentionsStore;
  inputRef: RefObject<HTMLTextAreaElement | null>;
  /** False while the box is not for a message (an agent waits for an answer): no "@" opens anything. */
  enabled: boolean;
};

/** The mentions of the text in the box, as the store holds them (re-renders when they change). */
export function useBoxMentions(store: MentionsStore): readonly Mention[] {
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  return store.current;
}

/**
 * The autocomplete of the composer (the ARIA 1.2 combobox pattern, with the focus staying in the
 * textarea): typing an "@" that begins a word opens a list of the agents that may be mentioned,
 * narrowed by what follows; the arrows move through it (`aria-activedescendant` names the active
 * option), Enter or Tab picks, Escape closes it until the word changes, and a click picks too.
 * A pick writes the agent's label into the text and the reference into the store, which then moves
 * with every edit (`lib/store.ts`).
 *
 * The hook also keeps the store in step with the box's text, whoever changes it.
 */
export function useMentions({ agents, store, inputRef, enabled }: Args) {
  const aui = useAui();
  const text = useAuiState((s) => s.composer.text);
  const [caret, setCaret] = useState(0);
  const [cursor, setCursor] = useState<{ key: string; index: number }>({ key: "", index: 0 });
  const [dismissed, setDismissed] = useState("");
  const listboxId = useId();

  // the store follows the text (a send clears it, a refused send brings it back, an undo changes it)
  useEffect(() => {
    store.sync(text);
  }, [store, text]);
  // the caret is where the browser left it once the new text is drawn
  useEffect(() => {
    setCaret(inputRef.current?.selectionStart ?? text.length);
  }, [text, inputRef]);

  const taken = useBoxMentions(store);
  const trigger = useMemo(
    () => (enabled && agents.length > 0 ? triggerAt(text, caret) : null),
    [enabled, agents.length, text, caret],
  );
  const options = useMemo(() => {
    if (!trigger || taken.length >= MAX_MENTIONS) return [];
    const already = new Set(taken.map((m) => m.agentId));
    return matching(
      agents.filter((a) => mentionable(a.id) && !already.has(a.id)),
      trigger.query,
    );
  }, [trigger, taken, agents]);
  const key = trigger ? `${trigger.start}:${trigger.query}` : "";
  const open = trigger !== null && options.length > 0 && dismissed !== key;
  const active = cursor.key === key ? Math.min(cursor.index, options.length - 1) : 0;
  const optionId = useCallback((i: number) => `${listboxId}-${i}`, [listboxId]);

  const pick = useCallback(
    (agent: ApiAgent) => {
      const el = inputRef.current;
      const now = aui.composer().getState().text;
      const at = el?.selectionStart ?? caret;
      const found = triggerAt(now, at);
      if (!found) return;
      const inserted = insertMention(now, found, at, agent);
      const moved = reconcile(now, inserted.text, store.current);
      store.set(
        inserted.text,
        [...moved, inserted.mention].sort((a, b) => a.start - b.start),
      );
      aui.composer().setText(inserted.text);
      setCaret(inserted.caret);
      // the controlled value was just replaced: the caret goes where the label ends, once it is drawn
      requestAnimationFrame(() => {
        el?.focus();
        el?.setSelectionRange(inserted.caret, inserted.caret);
      });
    },
    [aui, caret, inputRef, store],
  );

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!open || e.nativeEvent.isComposing) return;
    const move = (index: number) => {
      e.preventDefault();
      setCursor({ key, index: (index + options.length) % options.length });
    };
    if (e.key === "ArrowDown") return move(active + 1);
    if (e.key === "ArrowUp") return move(active - 1);
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
    if ((e.key === "Enter" && !e.shiftKey && plain) || (e.key === "Tab" && !e.shiftKey && plain)) {
      const agent = options[active];
      if (!agent) return;
      e.preventDefault();
      pick(agent);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setDismissed(key);
    }
  };

  /** Takes a mention off: its label leaves the text, and with it the reference. */
  const remove = (mention: Mention) => {
    aui.composer().setText(withoutMention(aui.composer().getState().text, mention));
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  return {
    open,
    options,
    active,
    listboxId,
    optionId,
    pick,
    remove,
    onKeyDown,
    /** The textarea's caret moved (a click, an arrow, a selection): read where it is. */
    onSelect: () => setCaret(inputRef.current?.selectionStart ?? 0),
    /** Mouse over an option makes it the active one. */
    hover: (index: number) => setCursor({ key, index }),
    /**
     * ARIA attributes of the textarea; none when no agent can be mentioned here. The box is a plain
     * textbox (named "Message", which is what every screen and test finds it by) that says it has
     * a list of suggestions; while the list is open it is the combobox of the ARIA 1.2 pattern, with
     * `aria-expanded`, `aria-controls` and `aria-activedescendant`.
     */
    comboboxProps:
      enabled && agents.length > 0
        ? open
          ? ({
              role: "combobox",
              "aria-autocomplete": "list",
              "aria-haspopup": "listbox",
              "aria-expanded": true,
              "aria-controls": listboxId,
              "aria-activedescendant": optionId(active),
            } as const)
          : ({ "aria-autocomplete": "list", "aria-haspopup": "listbox" } as const)
        : {},
  };
}
