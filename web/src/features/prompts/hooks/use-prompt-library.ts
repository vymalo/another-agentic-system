import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type {
  ApiCreatePrompt,
  ApiPrompt,
  ApiPromptSummary,
  ApiUpdatePrompt,
} from "@/lib/api/types";
import {
  type Context,
  type DeleteFailure,
  deleteFailureOf,
  type Failure,
  failureOf,
  type Problem,
  type ReadFailure,
} from "../lib/failures";
import { sortPrompts } from "../lib/library";
import { problemOf, readAllPrompts, readPrompt } from "../lib/read";

/*
 * The person's prompt library (`/api/prompts`, ADR 0070; the recipe is in `docs/api/chat-api.yaml`, on `listPrompts`). The list is
 * read page by page until the server has no more, when the page opens and again every time the window regains focus (a prompt
 * changed in another tab or by another client shows here). A write answers with the prompt as it is now, which replaces its row;
 * a write that failed, or whose outcome is not known, is followed by a read. Nothing is kept on the device: the library is the
 * server's, and another client sees exactly what this one does.
 */

export type LibraryState =
  | { status: "loading" }
  | {
      status: "ready";
      /** Own and shared, sorted by name then id. */
      prompts: ApiPromptSummary[];
      /** Why the last read failed, beside the list it left on screen; null after a good read. */
      readError: string | null;
    }
  /** 404: the deployment has no library. A fact, not a failure. */
  | { status: "off" }
  /** 403: the person's roles grant nothing at all. */
  | { status: "no-access" }
  /** 401 that the app's own session handling did not take. */
  | { status: "signed-out" }
  | { status: "error"; message: string };

export type WriteResult<T> = { ok: true; value: T } | { ok: false; failure: Failure };
export type ReadResult<T> = { ok: true; value: T } | { ok: false; failure: ReadFailure };
export type DeleteResult = { ok: true } | { ok: false; failure: DeleteFailure };

/** What the failure's words may name: the values as typed, since a problem never quotes one. */
export type Shown = Pick<Context, "name" | "body" | "ids">;

export type PromptLibraryView = {
  state: LibraryState;
  /** Reads the list again now (a failed first read, the person's "Try again"). */
  reload: () => void;
  /** The whole prompt (`getPrompt`): the form reads it to edit, the shared view to show. */
  get: (id: string) => Promise<ReadResult<ApiPrompt>>;
  create: (body: ApiCreatePrompt, shown: Shown) => Promise<WriteResult<ApiPrompt>>;
  /** `body.revision` is the one the form was opened at. */
  update: (id: string, body: ApiUpdatePrompt, shown: Shown) => Promise<WriteResult<ApiPrompt>>;
  /** A prompt that is already gone is deleted. */
  remove: (id: string, revision: string) => Promise<DeleteResult>;
};

type Limits = { limit: number; maxVariables: number };

const summaryOf = (p: ApiPrompt): ApiPromptSummary => ({
  id: p.id,
  name: p.name,
  title: p.title,
  description: p.description,
  variableCount: p.variables.length,
  own: p.own,
  ...(p.audience ? { audience: p.audience } : {}),
  revision: p.revision,
  updatedAt: p.updatedAt,
});

export function usePromptLibrary(config: Limits): PromptLibraryView {
  const [state, setState] = useState<LibraryState>({ status: "loading" });
  const asked = useRef(0);
  const settled = useRef(0);
  const alive = useRef(true);
  const shown = useRef(false);
  const lastRead = useRef(0);
  const limits = useRef(config);
  limits.current = config;

  const load = useCallback(() => {
    const mine = ++asked.current;
    lastRead.current = Date.now();
    // a read that is overtaken by a newer one is dropped, so a slow answer never replaces a fresher list
    const apply = (next: LibraryState) => {
      if (!alive.current || mine < settled.current) return;
      settled.current = mine;
      setState(next);
    };
    const later = (readError: string) => {
      if (!alive.current || mine < settled.current) return;
      settled.current = mine;
      setState((s) => (s.status === "ready" ? { ...s, readError } : s));
    };
    readAllPrompts()
      .then((read) => {
        if (read.ok) {
          shown.current = true;
          apply({ status: "ready", prompts: sortPrompts(read.items), readError: null });
        } else if (shown.current) {
          // a list on screen stays when a later read fails, whatever the failure: it is said beside it
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
    load();
    // a tab that is switched to fires `visibilitychange` and `focus` together: one read, and none within a quarter second of another
    const again = () => {
      if (document.visibilityState === "hidden" || Date.now() - lastRead.current < 250) return;
      load();
    };
    window.addEventListener("focus", again);
    document.addEventListener("visibilitychange", again);
    return () => {
      alive.current = false;
      window.removeEventListener("focus", again);
      document.removeEventListener("visibilitychange", again);
    };
  }, [load]);

  const put = useCallback((prompt: ApiPrompt) => {
    // a read that began before this write may carry what it changed: it is dropped when it answers
    settled.current = ++asked.current;
    const row = summaryOf(prompt);
    setState((s) =>
      s.status === "ready"
        ? { ...s, prompts: sortPrompts([...s.prompts.filter((x) => x.id !== row.id), row]) }
        : s,
    );
  }, []);

  /** A refused or failed write, in words; the list is read again where the outcome is not what the person asked for. */
  const failed = useCallback(
    (problem: Problem, shownValues: Shown): { ok: false; failure: Failure } => {
      const failure = failureOf(problem, {
        limit: limits.current.limit,
        maxVariables: limits.current.maxVariables,
        ...shownValues,
      });
      if (failure.reread) load();
      return { ok: false, failure };
    },
    [load],
  );

  const create = useCallback<PromptLibraryView["create"]>(
    async (body, values) => {
      try {
        const { data, error, response } = await api.POST("/api/prompts", {
          body: body as never,
        });
        if (data) {
          put(data);
          return { ok: true, value: data };
        }
        return failed(problemOf(error, response.status), values);
      } catch {
        return failed({ status: 0 }, values);
      }
    },
    [failed, put],
  );

  const update = useCallback<PromptLibraryView["update"]>(
    async (id, body, values) => {
      try {
        const { data, error, response } = await api.PATCH("/api/prompts/{promptId}", {
          params: { path: { promptId: id } },
          body: body as never,
        });
        if (data) {
          put(data);
          return { ok: true, value: data };
        }
        return failed(problemOf(error, response.status), values);
      } catch {
        return failed({ status: 0 }, values);
      }
    },
    [failed, put],
  );

  const remove = useCallback<PromptLibraryView["remove"]>(
    async (id, revision) => {
      let problem: Problem;
      try {
        const { error, response } = await api.DELETE("/api/prompts/{promptId}", {
          params: { path: { promptId: id }, query: { revision } },
        });
        problem = problemOf(error, response.status);
        // a second delete is a 404: the prompt is gone, which is what was asked
        if (response.ok || response.status === 404) {
          settled.current = ++asked.current;
          setState((s) =>
            s.status === "ready" ? { ...s, prompts: s.prompts.filter((x) => x.id !== id) } : s,
          );
          return { ok: true };
        }
      } catch {
        problem = { status: 0 };
      }
      const failure = deleteFailureOf(problem);
      if (failure.reread) load();
      return { ok: false, failure };
    },
    [load],
  );

  const get = useCallback<PromptLibraryView["get"]>(async (id) => readPrompt(id), []);

  return { state, reload: load, get, create, update, remove };
}
