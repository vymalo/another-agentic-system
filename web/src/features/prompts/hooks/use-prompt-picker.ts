import { useCallback, useEffect, useRef, useState } from "react";
import type { Answer, ChoicesSpec } from "@/features/chat/lib/a2ui/choices";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiPromptSummary } from "@/lib/api/types";
import { type FillFailure, fillFailureOf, fillProblemOf } from "../lib/fill";
import { sortPrompts } from "../lib/library";
import { type PromptListRead, type PromptRead, readAllPrompts, readPrompt } from "../lib/read";

/*
 * The prompts of the `/` menu (docs/api/prompts-v1.md, "The recipe"): the list is read when the menu
 * first opens and again every time it reopens, never on a thread's mount (a library of 100 prompts is
 * not read for a message that names none). A prompt is read whole when it is chosen, and filled with
 * `renderPrompt`; nothing is kept on the device.
 */

export type PickerState =
  | { status: "idle" }
  | { status: "loading" }
  | {
      status: "ready";
      /** Own and shared, sorted by name then id. */
      prompts: ApiPromptSummary[];
      /** Why the last read failed, beside the list it left on screen; null after a good read. */
      readError: string | null;
    }
  /** 404: the deployment has no library. */
  | { status: "off" }
  /** 403: the person's roles grant nothing at all. */
  | { status: "no-access" }
  /** 401 that the app's own session handling did not take. */
  | { status: "signed-out" }
  | { status: "error"; message: string };

export type RenderResult =
  | { ok: true; text: string; revision: string }
  | { ok: false; failure: FillFailure };

export type PromptPicker = {
  state: PickerState;
  /** `getPrompt`: the whole prompt, for the form. */
  get: (id: string) => Promise<PromptRead>;
  /** `renderPrompt`: the text for the answers, or the words of the refusal. */
  render: (
    id: string,
    revision: string,
    answers: Answer[],
    spec: ChoicesSpec | undefined,
    maxRenderedBytes: number,
  ) => Promise<RenderResult>;
};

/** What the picker needs of `getConfig.prompts`: its presence is the capability, and the render limit is its words. */
type Limits = { maxRenderedBytes: number };

/**
 * The list behind the `/` menu. `open` is whether the menu is open: a read starts when it becomes
 * true, so a thread that never types `/` asks `/api/prompts` nothing. A list already on screen stays
 * while a later read is in flight, and a read that fails says so beside it.
 */
export function usePromptPicker({
  config,
  open,
}: {
  config: Limits | undefined;
  open: boolean;
}): PromptPicker {
  const [state, setState] = useState<PickerState>({ status: "idle" });
  const asked = useRef(0);
  const settled = useRef(0);
  const alive = useRef(true);
  const shown = useRef(false);

  const load = useCallback(() => {
    const mine = ++asked.current;
    const apply = (next: PickerState) => {
      if (!alive.current || mine < settled.current) return;
      settled.current = mine;
      setState(next);
    };
    const later = (readError: string) => {
      if (!alive.current || mine < settled.current) return;
      settled.current = mine;
      setState((s) => (s.status === "ready" ? { ...s, readError } : s));
    };
    setState((s) => (s.status === "ready" ? { ...s, readError: null } : { status: "loading" }));
    readAllPrompts()
      .then((read: PromptListRead) => {
        if (read.ok) {
          shown.current = true;
          apply({ status: "ready", prompts: sortPrompts(read.items), readError: null });
        } else if (shown.current) {
          later(
            read.status === 401
              ? "You are signed out. Sign in again to see changes."
              : problemMessage(read.error),
          );
        } else if (read.status === 404) apply({ status: "off" });
        else if (read.status === 403) apply({ status: "no-access" });
        else if (read.status === 401) apply({ status: "signed-out" });
        else apply({ status: "error", message: problemMessage(read.error) });
      })
      .catch((e: unknown) => {
        if (shown.current) later(problemMessage(e));
        else apply({ status: "error", message: problemMessage(e) });
      });
  }, []);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    if (!config) {
      setState({ status: "off" });
      return;
    }
    load();
  }, [open, config, load]);

  const get = useCallback<PromptPicker["get"]>(
    async (id) => {
      const read = await readPrompt(id);
      // a prompt that is gone (or no longer shared) leaves the list: the next `/` does not offer it
      if (!read.ok && read.failure.gone) load();
      return read;
    },
    [load],
  );

  const render = useCallback<PromptPicker["render"]>(
    async (id, revision, answers, spec, maxRenderedBytes) => {
      try {
        const { data, error, response } = await api.POST("/api/prompts/{promptId}/render", {
          params: { path: { promptId: id } },
          body: { revision, answers },
        });
        if (data) return { ok: true, text: data.text, revision: data.revision };
        if (response.status === 404) load();
        return {
          ok: false,
          failure: fillFailureOf(fillProblemOf(error, response.status), spec, maxRenderedBytes),
        };
      } catch {
        return { ok: false, failure: fillFailureOf({ status: 0 }, spec, maxRenderedBytes) };
      }
    },
    [load],
  );

  return { state, get, render };
}
