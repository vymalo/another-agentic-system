import { type AgUiAssistantRuntime, useAgUiRuntime } from "@assistant-ui/react-ag-ui";
import { useRouter } from "next/navigation";
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { useElapsed } from "@/features/chat/hooks/use-elapsed";
import { dropFailedSend } from "@/features/chat/lib/agui/failed-send";
import {
  type SendError,
  type Target,
  ThreadAgent,
  type ThreadSnapshot,
} from "@/features/chat/lib/agui/thread-agent";
import type { ThreadsView } from "@/features/threads/hooks/use-threads";
import { isTerminal } from "@/lib/api/types";
import { uuidv7 } from "@/lib/uuid";

export type Selection = { agentId: string | null; release: string | null };

type Args = {
  /** null on the new-thread page: the agent mints the id of the thread its first send creates. */
  threadId: string | null;
  /** Where a send goes: the open thread's agent, or the new-thread page's selection. */
  target: Selection;
  threads: ThreadsView;
  /** `lastSeq` of `GET /api/threads/{id}`: how far the conversation is; null until fetched. */
  threadLastSeq: number | null;
  notFound: boolean;
  onSendFailed: (message: string, status: number | undefined) => void;
  onSending: () => void;
};

export type ChatRuntime = {
  runtime: AgUiAssistantRuntime;
  agent: ThreadAgent;
  snapshot: ThreadSnapshot;
  /** The conversation is caught up with the server (sticky: a later event does not unload it). */
  loaded: boolean;
};

const newThreadId = (): string => uuidv7();

/**
 * How long a finished thread keeps following its stream. The orchestrator's title and description
 * (ADR 0035) are written by a model once a job has ended, each a call of up to its endpoint's
 * timeout (30 s by default) and a retry or two, so a thread that is already `done` still changes.
 */
export const FINISHED_GRACE_MS = 45_000;

/**
 * `@assistant-ui/react-ag-ui` over one `ThreadAgent` (ADR 0006, ADR 0012).
 *
 * The conversation is AG-UI: the agent follows `GET /agui/threads/{id}/connect`, a send is
 * `POST /agui/agents/{agentId}`. What the pages read besides (thread list, thread details) is the
 * REST resource API. `LiveRuns` (mounted inside the provider) feeds the runs the user did not start.
 */
export function useChatRuntime({
  threadId,
  target,
  threads,
  threadLastSeq,
  notFound,
  onSendFailed,
  onSending,
}: Args): ChatRuntime {
  const router = useRouter();
  const targetRef = useRef<Target>(target);
  targetRef.current = target;
  const onSendFailedRef = useRef(onSendFailed);
  onSendFailedRef.current = onSendFailed;
  const onSendingRef = useRef(onSending);
  onSendingRef.current = onSending;
  const runtimeRef = useRef<AgUiAssistantRuntime | null>(null);

  const agent = useMemo(
    () =>
      new ThreadAgent({
        threadId: threadId ?? newThreadId(),
        target: () => targetRef.current,
        onSending: () => onSendingRef.current(),
        // The first send of the new-thread page creates the thread: go to it.
        onAccepted: ({ threadId: id }) => {
          if (threadId === null) router.push(`/threads/${id}`);
        },
      }),
    [threadId, router],
  );

  const snapshot = useSyncExternalStore(agent.onChange, agent.getSnapshot, agent.getSnapshot);

  const caughtUp = threadLastSeq !== null && snapshot.lastSeq >= threadLastSeq;
  // A finished thread that is fully loaded needs no stream for long, and neither does one that is not
  // there. "For long": what the orchestrator writes after a job ends, its title and the thread's
  // description (ADR 0035), arrives on the stream after the thread is `done`, so a thread this page
  // watched finish keeps its stream for a while (`FINISHED_GRACE_MS`) before it lets go. One that was
  // already finished when it was opened lets go at once: its job ended before, and so did what follows it.
  const finished = isTerminal(snapshot.state) && caughtUp && !snapshot.openRun;
  const watched = useRef<{ agent: ThreadAgent; unfinished: boolean }>({ agent, unfinished: false });
  if (watched.current.agent !== agent) watched.current = { agent, unfinished: false };
  if (caughtUp && !finished) watched.current.unfinished = true;
  const lingered = useElapsed(finished && watched.current.unfinished, FINISHED_GRACE_MS);
  const paused =
    notFound || snapshot.notFound || (finished && (!watched.current.unfinished || lingered));
  useEffect(() => {
    if (threadId === null || paused) return;
    agent.start();
    return () => agent.stop();
  }, [agent, threadId, paused]);

  const [loaded, setLoaded] = useState(threadId === null);
  useEffect(() => {
    if (caughtUp || notFound || snapshot.notFound) setLoaded(true);
  }, [caughtUp, notFound, snapshot.notFound]);

  const runtime = useAgUiRuntime({
    agent,
    resumeTranscript: "appended",
    // A run that ends in RUN_ERROR reports through here too; only a refused send is a send error
    // (the rest of a failed run is in the transcript as its status lines).
    onError: () => {
      const failure: SendError | null = agent.takeSendError();
      const current = runtimeRef.current;
      if (!failure || !current) return;
      // the composer takes its text back by itself (the failure is a MessageNotSentError); an
      // A2UI action carried no message, so there is nothing to take back
      if (!failure.action) dropFailedSend(current);
      onSendFailedRef.current(failure.message, failure.status);
    },
    adapters: {
      threadList: {
        ...(threadId !== null ? { threadId } : {}),
        isLoading: threads.loading,
        threads: threads.threads.map((t) => ({
          status: "regular" as const,
          id: t.id,
          title: t.title,
        })),
        onSwitchToNewThread: () => router.push("/"),
        onSwitchToThread: (id: string) => {
          router.push(`/threads/${id}`);
          return { messages: [] };
        },
      },
    },
  });
  runtimeRef.current = runtime;

  return { runtime, agent, snapshot, loaded };
}
