import { api } from "@/lib/api/client";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";
import { type Problem, type ReadFailure, readFailureOf } from "./failures";

/*
 * Reading a person's prompt library (`/api/prompts`, ADR 0070) for the two screens that use it: the
 * page of the library and the `/` menu of the message box. The list is read page by page until the
 * server names no next one; a prompt is read whole by its id. The words of a refusal are in
 * `failures.ts`.
 */

/** The members of a problem body this app reads: its status, its `code` and where it points. */
export const problemOf = (error: unknown, status: number): Problem => {
  const e = (error ?? {}) as { code?: unknown; at?: unknown; offset?: unknown };
  return {
    status,
    ...(typeof e.code === "string" ? { code: e.code } : {}),
    ...(typeof e.at === "string" ? { at: e.at } : {}),
    ...(typeof e.offset === "number" ? { offset: e.offset } : {}),
  };
};

export type PromptListRead =
  | { ok: true; items: ApiPromptSummary[] }
  | { ok: false; status: number; error: unknown };

/** Every page of the list, in the order the server gave them, until it names no next one (or one it already gave). */
export async function readAllPrompts(): Promise<PromptListRead> {
  // by id: a prompt written between two pages must not be listed twice
  const items = new Map<string, ApiPromptSummary>();
  const seen = new Set<string>();
  let cursor: string | undefined;
  for (;;) {
    const { data, error, response } = await api.GET("/api/prompts", {
      params: { query: cursor === undefined ? {} : { cursor } },
    });
    if (!data) return { ok: false, status: response.status, error };
    for (const item of data.items) items.set(item.id, item);
    const next = data.nextCursor;
    if (!next || seen.has(next)) return { ok: true, items: [...items.values()] };
    seen.add(next);
    cursor = next;
  }
}

export type PromptRead = { ok: true; value: ApiPrompt } | { ok: false; failure: ReadFailure };

/** `getPrompt`: the whole prompt, with its variables and revision. */
export async function readPrompt(id: string): Promise<PromptRead> {
  try {
    const { data, error, response } = await api.GET("/api/prompts/{promptId}", {
      params: { path: { promptId: id } },
    });
    if (data) return { ok: true, value: data };
    return { ok: false, failure: readFailureOf(problemOf(error, response.status)) };
  } catch {
    return { ok: false, failure: readFailureOf({ status: 0 }) };
  }
}
