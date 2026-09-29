import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { applyEvents, emptyLog, lastThreadStateEvent } from "@/features/chat/lib/event-log";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiEvent, ApiThread, ThreadState } from "@/lib/api/types";
import { isTerminal } from "@/lib/api/types";
import { type Connection, useEventStream } from "./use-event-stream";

export type ThreadView = {
  log: ReturnType<typeof emptyLog>;
  thread: ApiThread | null;
  /** Server-derived state: a `thread_state` event newer than the last fetch, else the fetch. */
  state: ThreadState | undefined;
  loaded: boolean;
  notFound: boolean;
  error: string | null;
  connection: Connection;
  /** Feed events known to be real (server responses), e.g. the 202 body of a follow-up. */
  addEvents: (events: readonly ApiEvent[]) => void;
  reload: () => void;
};

const REFETCH_DEBOUNCE_MS = 150;
const REFETCH_KINDS = new Set(["user_message", "agent_status", "thread_state"]);

/** One thread: its event log (via SSE), its metadata (via GET) and the state derived from both. */
export function useThread(threadId: string | null): ThreadView {
  const [log, setLog] = useState(() => emptyLog(threadId ?? ""));
  const [thread, setThread] = useState<ApiThread | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const refetchTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const fetchThread = useCallback(async () => {
    if (threadId === null) return;
    const {
      data,
      error: err,
      response,
    } = await api.GET("/api/threads/{threadId}", {
      params: { path: { threadId } },
    });
    if (data) {
      setThread(data);
      setError(null);
      setNotFound(false);
    } else if (response.status === 404) {
      setNotFound(true);
    } else {
      setError(problemMessage(err));
    }
  }, [threadId]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: reloadKey re-runs the initial fetch
  useEffect(() => {
    void fetchThread().catch((e: unknown) => setError(problemMessage(e)));
    return () => clearTimeout(refetchTimer.current);
  }, [fetchThread, reloadKey]);

  const addEvents = useCallback(
    (events: readonly ApiEvent[]) => {
      setLog((prev) => applyEvents(prev, events));
      if (events.some((e) => REFETCH_KINDS.has(e.kind))) {
        clearTimeout(refetchTimer.current);
        refetchTimer.current = setTimeout(() => {
          void fetchThread().catch(() => {});
        }, REFETCH_DEBOUNCE_MS);
      }
    },
    [fetchThread],
  );

  const stateEvent = lastThreadStateEvent(log);
  const state = useMemo<ThreadState | undefined>(() => {
    if (stateEvent && (!thread || stateEvent.seq > thread.lastSeq)) return stateEvent.state;
    return thread?.state ?? stateEvent?.state;
  }, [stateEvent, thread]);

  // A finished thread that is fully loaded needs no stream. Its 404/error also stops it.
  const caughtUp = thread !== null && log.lastSeq >= thread.lastSeq;
  const enabled = threadId !== null && !notFound && !(isTerminal(state) && caughtUp);
  const connection = useEventStream(threadId ?? "", { enabled, onEvents: addEvents });

  return {
    log,
    thread,
    state,
    loaded: threadId === null || thread !== null || notFound,
    notFound,
    error,
    connection,
    addEvents,
    reload: () => setReloadKey((k) => k + 1),
  };
}
