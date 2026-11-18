"use client";

import { useAui, useAuiState } from "@assistant-ui/react";
import {
  type KeyboardEvent,
  type RefObject,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { usePromptsConfig } from "@/features/chat/hooks/use-ui-config";
import type { Answer, ChoicesSpec } from "@/features/chat/lib/a2ui/choices";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";
import { matchingPrompts, promptQueryOf, specOf } from "../lib/fill";
import type { PromptRead } from "../lib/read";
import { type PickerState, type RenderResult, usePromptPicker } from "./use-prompt-picker";

type Notice = { tone: "muted" | "error"; text: string };

type Args = {
  /** False while the box is not for a message (an agent waits for an answer): no "/" opens anything. */
  enabled: boolean;
  inputRef: RefObject<HTMLTextAreaElement | null>;
};

/**
 * The `/` menu of the message box (docs/api/prompts-v1.md, "The recipe"): a message that is one
 * `/name` opens the prompts the person owns and the ones shared with them; Enter or Tab picks one,
 * Escape closes the list until the query changes, and a click picks too. Picking reads the prompt
 * and, when it has questions, opens the fill form; the text `renderPrompt` answers with goes into
 * the box for the person to edit. **Nothing here sends a message.**
 *
 * The list is read when the menu first opens and again each time it reopens, never on a thread's
 * mount. The focus never leaves the textarea: the listbox is the combobox's, and the form is a
 * dialog.
 */
export function usePromptMenu({ enabled, inputRef }: Args) {
  const aui = useAui();
  const text = useAuiState((s) => s.composer.text);
  const config = usePromptsConfig();
  const trigger = useMemo(
    () => (enabled && config ? promptQueryOf(text) : null),
    [enabled, config, text],
  );
  const open = trigger !== null;
  const picker = usePromptPicker({ config, open });
  const { state: pickerState, get, render } = picker;

  const [dismissed, setDismissed] = useState<string | null>(null);
  const [cursor, setCursor] = useState<{ key: string; index: number }>({ key: "", index: 0 });
  const [notice, setNotice] = useState<Notice | null>(null);
  const [filling, setFilling] = useState<ApiPrompt | null>(null);
  const listboxId = useId();
  const picked = useRef(0);
  const pickedKey = useRef("");

  const key = trigger?.query ?? "";
  const options = useMemo(
    () =>
      trigger
        ? matchingPrompts(pickerState.status === "ready" ? pickerState.prompts : [], trigger.query)
        : [],
    [trigger, pickerState],
  );
  const shown = trigger !== null && dismissed !== key;
  const active = cursor.key === key ? Math.min(cursor.index, Math.max(options.length - 1, 0)) : 0;
  const optionId = useCallback((i: number) => `${listboxId}-${i}`, [listboxId]);

  // the box stopped being one "/name": the next one opens the list afresh (the same prompt can be
  // used again, and a command that was dismissed is not remembered forever)
  useEffect(() => {
    if (!trigger) setDismissed(null);
  }, [trigger]);

  const focusBox = useCallback(
    () => requestAnimationFrame(() => inputRef.current?.focus()),
    [inputRef],
  );

  /** Puts the rendered text in the box, the caret after it: the person edits and sends it. */
  const insert = useCallback(
    (next: string) => {
      aui.composer().setText(next);
      requestAnimationFrame(() => {
        const el = inputRef.current;
        el?.focus();
        el?.setSelectionRange(next.length, next.length);
      });
    },
    [aui, inputRef],
  );

  /** The `/query` comes back when a pick could not be filled and the box is still empty; the list opens again for it. */
  const restore = useCallback(
    (query: string) => {
      if (aui.composer().getState().text === "") aui.composer().setText(`/${query}`);
      setDismissed(null);
      focusBox();
    },
    [aui, focusBox],
  );

  const pick = useCallback(
    async (summary: ApiPromptSummary) => {
      if (!trigger) return;
      const mine = ++picked.current;
      const query = trigger.query;
      pickedKey.current = query;
      setNotice(null);
      const composer = aui.composer();
      composer.setText("");
      // a pick that is overtaken by another, or whose box was written in while it was read, is dropped
      const dropped = () => {
        if (mine !== picked.current) return true;
        if (aui.composer().getState().text === "") return false;
        setNotice(null);
        return true;
      };
      setNotice({ tone: "muted", text: `Reading “${summary.title}”…` });
      const read = await get(summary.id);
      if (dropped()) return;
      if (!read.ok) {
        setNotice({ tone: "error", text: read.failure.message });
        restore(query);
        return;
      }
      const prompt = read.value;
      const spec = specOf(prompt);
      if (!spec) {
        setNotice({
          tone: "error",
          text: "This prompt asks a kind of question this client cannot draw, so it is not offered.",
        });
        restore(query);
        return;
      }
      if (prompt.variables.length === 0) {
        setNotice({ tone: "muted", text: `Filling “${prompt.title}”…` });
        const result = await render(
          prompt.id,
          prompt.revision,
          [],
          spec,
          config?.maxRenderedBytes ?? 0,
        );
        if (dropped()) return;
        if (!result.ok) {
          setNotice({ tone: "error", text: result.failure.message });
          restore(query);
          return;
        }
        setNotice(null);
        insert(result.text);
        return;
      }
      setNotice(null);
      setFilling(prompt);
    },
    [aui, config, get, insert, render, restore, trigger],
  );

  const fill = useCallback(
    async (revision: string, answers: Answer[], spec: ChoicesSpec): Promise<RenderResult> => {
      if (!filling) {
        return {
          ok: false,
          failure: { message: "The prompt could not be filled just now. Try again in a moment." },
        };
      }
      return render(filling.id, revision, answers, spec, config?.maxRenderedBytes ?? 0);
    },
    [config, filling, render],
  );

  const loadCurrent = useCallback(async (): Promise<PromptRead> => {
    if (!filling) {
      return {
        ok: false,
        failure: {
          message: "The prompt could not be loaded just now. Try again in a moment.",
          reread: false,
          gone: false,
        },
      };
    }
    return get(filling.id);
  }, [filling, get]);

  const filled = useCallback(
    (next: string) => {
      setFilling(null);
      setNotice(null);
      insert(next);
    },
    [insert],
  );

  /** The form is closed: the command comes back in the box, and the list opens again for it. */
  const close = useCallback(() => {
    setFilling(null);
    restore(pickedKey.current);
  }, [restore]);

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!shown || e.nativeEvent.isComposing) return;
    const move = (index: number) => {
      e.preventDefault();
      setCursor({ key, index: (index + options.length) % options.length });
    };
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (options.length === 0) return;
      move(active + (e.key === "ArrowDown" ? 1 : -1));
      return;
    }
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
    if ((e.key === "Enter" && !e.shiftKey && plain) || (e.key === "Tab" && !e.shiftKey && plain)) {
      if (options.length === 0) {
        // a half-typed command is not sent while the list is still being read; Tab may leave the box
        if (
          e.key === "Enter" &&
          (pickerState.status === "loading" || pickerState.status === "idle")
        ) {
          e.preventDefault();
        }
        return;
      }
      const option = options[active];
      if (!option) return;
      e.preventDefault();
      void pick(option);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setDismissed(key);
    }
  };

  const comboboxProps =
    enabled && config
      ? shown
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
    open: shown,
    state: pickerState,
    query: key,
    options,
    active,
    listboxId,
    optionId,
    onKeyDown,
    comboboxProps,
    notice,
    filling,
    pick,
    fill,
    loadCurrent,
    filled,
    close,
    hover: (index: number) => setCursor({ key, index }),
  };
}

export type PromptMenuView = ReturnType<typeof usePromptMenu>;
export type { PickerState };
