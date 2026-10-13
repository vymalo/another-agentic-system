import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import { type ApiThread, isActive, type ThreadState } from "@/lib/api/types";

/** How long "Stop and delete" waits for the agent to report the stop, and how often it asks. */
export type EraseTiming = { pollMs: number; giveUpMs: number };
export const ERASE_TIMING: EraseTiming = { pollMs: 400, giveUpMs: 30_000 };

export type EraseState =
  | { phase: "idle" }
  | { phase: "erasing" }
  | { phase: "stopping" }
  | { phase: "failed"; message: string };

export type ThreadEraser = {
  state: EraseState;
  /** The thread works (the list said so, or the server refused with `thread_active`): it has to stop first. */
  needsStop: boolean;
  /** `deleteThread`; a 409 `thread_active` makes `needsStop` true. */
  erase: () => void;
  /** `cancelThread`, wait for a state that is not working (bounded), then `deleteThread`. */
  stopAndErase: () => void;
  /** Gives up whatever is waiting; nothing more is sent. */
  abort: () => void;
};

type Outcome =
  | { kind: "erased" }
  | { kind: "gone" }
  | { kind: "active" }
  | { kind: "failed"; message: string };

const codeOf = (error: unknown): string | undefined =>
  typeof error === "object" && error !== null && "code" in error
    ? String((error as { code: unknown }).code)
    : undefined;

/** One `DELETE /api/threads/{id}` (ADR 0043), as the dialog needs its answer. */
async function deleteOnce(id: string): Promise<Outcome> {
  try {
    const { error, response } = await api.DELETE("/api/threads/{threadId}", {
      params: { path: { threadId: id } },
    });
    if (response.ok) return { kind: "erased" };
    // already erased (another tab, or a second click): the list drops it
    if (response.status === 404) return { kind: "gone" };
    if (response.status === 409 && codeOf(error) === "thread_active") return { kind: "active" };
    if (response.status === 403) {
      return { kind: "failed", message: "Your roles do not let you delete chats." };
    }
    return { kind: "failed", message: `Could not delete the chat: ${problemMessage(error)}` };
  } catch (e) {
    return { kind: "failed", message: `Could not delete the chat: ${problemMessage(e)}` };
  }
}

/** Where the thread stands (`GET /api/threads/{id}`): gone (404), its state, or nothing when it could not be read. */
async function readState(id: string): Promise<{ gone: boolean; state?: ThreadState }> {
  try {
    const { data, response } = await api.GET("/api/threads/{threadId}", {
      params: { path: { threadId: id } },
    });
    return { gone: response.status === 404, ...(data ? { state: data.state } : {}) };
  } catch {
    // a read that failed is asked again until the time is up
    return { gone: false };
  }
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Deleting a thread (ADR 0043) and, when it works, stopping it first: the server refuses a delete
 * while the thread is `queued`, `working` or `verifying` (the unsent request to stop the agent would
 * go with the row, and the agent would go on), so "Stop and delete" cancels, waits until the thread is
 * in a state that is not working (the stop is a request, not a state: it ends when the agent reports),
 * and only then deletes. Every wait is bounded and ends with words; `abort` ends it without a delete.
 */
export function useEraseThread(
  thread: ApiThread,
  { onErased, timing }: { onErased: (id: string) => void; timing: EraseTiming },
): ThreadEraser {
  const [state, setState] = useState<EraseState>({ phase: "idle" });
  const [working, setWorking] = useState(isActive(thread.state));
  /** Each flow takes a number; a flow that is not the latest (or the unmount) is stale and writes nothing. */
  const epoch = useRef(0);
  useEffect(() => {
    return () => {
      epoch.current += 1;
    };
  }, []);

  const id = thread.id;

  /** Deletes, then says what came of it; `again` is what a thread that works again is told. */
  const finish = useCallback(
    async (mine: number, again: string) => {
      const outcome = await deleteOnce(id);
      if (epoch.current !== mine) return;
      switch (outcome.kind) {
        case "erased":
        case "gone":
          onErased(id);
          return;
        case "active":
          setWorking(true);
          setState(again === "" ? { phase: "idle" } : { phase: "failed", message: again });
          return;
        case "failed":
          setState({ phase: "failed", message: outcome.message });
      }
    },
    [id, onErased],
  );

  const erase = useCallback(() => {
    const mine = ++epoch.current;
    setState({ phase: "erasing" });
    void finish(mine, "");
  }, [finish]);

  const stopAndErase = useCallback(() => {
    const mine = ++epoch.current;
    const stale = () => epoch.current !== mine;
    const fail = (message: string) => setState({ phase: "failed", message });
    setState({ phase: "stopping" });
    void (async () => {
      try {
        const { error, response } = await api.POST("/api/threads/{threadId}/cancel", {
          params: { path: { threadId: id } },
        });
        if (stale()) return;
        if (response.status === 404) return onErased(id);
        if (response.status === 403) {
          return fail(
            "Your roles do not let you stop chats, so this one cannot be stopped and deleted.",
          );
        }
        if (!response.ok) return fail(`Could not stop the chat: ${problemMessage(error)}`);
        const deadline = Date.now() + timing.giveUpMs;
        for (;;) {
          const read = await readState(id);
          if (stale()) return;
          if (read.gone) return onErased(id);
          if (read.state !== undefined && !isActive(read.state)) break;
          if (Date.now() >= deadline) {
            return fail("The agent has not stopped yet. Try again in a moment.");
          }
          await sleep(timing.pollMs);
          if (stale()) return;
        }
        await finish(mine, "The chat is working again. Stop it once more to delete it.");
      } catch (e) {
        if (!stale()) fail(`Could not stop the chat: ${problemMessage(e)}`);
      }
    })();
  }, [id, onErased, timing.giveUpMs, timing.pollMs, finish]);

  const abort = useCallback(() => {
    epoch.current += 1;
  }, []);

  return { state, needsStop: working, erase, stopAndErase, abort };
}
