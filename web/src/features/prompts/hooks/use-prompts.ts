"use client";

import { useAui } from "@assistant-ui/react";
import { type RefObject, useCallback, useRef, useState } from "react";
import { usePromptsConfig } from "@/features/chat/hooks/use-ui-config";
import type { Answer, ChoicesSpec } from "@/features/chat/lib/a2ui/choices";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";
import { specOf } from "../lib/fill";
import type { PromptRead } from "../lib/read";
import { type PickerState, type RenderResult, usePromptPicker } from "./use-prompt-picker";

type Notice = { tone: "muted" | "error"; text: string };

type Args = {
  /** False while the box is not for a message (an agent waits for an answer): nothing is read. */
  enabled: boolean;
  /** Whether the box is one `/name` right now: the list is read when it becomes one, and read again each time. */
  listing: boolean;
  inputRef: RefObject<HTMLTextAreaElement | null>;
};

/**
 * The person's half of the one `/` menu (docs/api/prompts-v1.md, "The recipe"): the prompts they own
 * and the ones shared with them, the pick that reads one and opens its form, and the text
 * `renderPrompt` answers with. **This hook holds no listbox and no keys** — `useSlashMenu` draws the
 * one menu the agent's commands and these prompts share, and hands it the pick.
 *
 * The list is read when the box becomes one `/name` and again each time it becomes one, never on a
 * thread's mount. `offered` is whether the deployment has a library at all (the configuration's
 * `prompts`); a menu draws the prompts' group only then.
 */
export function usePrompts({ enabled, listing, inputRef }: Args) {
  const aui = useAui();
  const config = usePromptsConfig();
  const picker = usePromptPicker({ config, open: enabled && listing });
  const { state: pickerState, get, render } = picker;

  const [notice, setNotice] = useState<Notice | null>(null);
  const [filling, setFilling] = useState<ApiPrompt | null>(null);
  const picked = useRef(0);
  const pickedKey = useRef("");

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

  /** The `/query` comes back when a pick could not be filled and the box is still empty; the menu opens again for it. */
  const restore = useCallback(
    (query: string) => {
      if (aui.composer().getState().text === "") aui.composer().setText(`/${query}`);
      focusBox();
    },
    [aui, focusBox],
  );

  /**
   * Picks a prompt for the `/query` the box holds: reads it and, when it has questions, opens the
   * fill form; the text `renderPrompt` answers with goes into the box. Nothing is sent.
   */
  const pick = useCallback(
    async (summary: ApiPromptSummary, query: string) => {
      const mine = ++picked.current;
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
    [aui, config, get, insert, render, restore],
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

  /** The form is closed: the command comes back in the box, and the menu opens again for it. */
  const close = useCallback(() => {
    setFilling(null);
    restore(pickedKey.current);
  }, [restore]);

  return {
    /** Whether the deployment offers a library at all: the menu draws the prompts' group only then. */
    offered: config !== undefined,
    state: pickerState,
    notice,
    filling,
    pick,
    fill,
    loadCurrent,
    filled,
    close,
  };
}

export type PromptsView = ReturnType<typeof usePrompts>;
export type { PickerState };
