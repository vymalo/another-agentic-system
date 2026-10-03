import { type AgUiAssistantRuntime, useAgUiRuntime } from "@assistant-ui/react-ag-ui";
import { useRouter } from "next/navigation";
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { useElapsed, useElapsedAt } from "@/features/chat/hooks/use-elapsed";
import { dropFailedSend } from "@/features/chat/lib/agui/failed-send";
import {
  type MentionsSource,
  type SendError,
  type Target,
  ThreadAgent,
  type ThreadSnapshot,
} from "@/features/chat/lib/agui/thread-agent";
import type { ShareSource } from "@/features/sharing/lib/sharing";
import type { ThreadsView } from "@/features/threads/hooks/use-threads";
import { isTerminal } from "@/lib/api/types";
import { uuidv7 } from "@/lib/uuid";

export type Selection = { agentId: string | null; release: string | null };

type Args = {
  /** null on the new-thread page: the agent mints the id of the thread its first send creates. */
  threadId: string | null;
  /**
   * Where a send goes: the open thread's agent, or the new-thread page's selection and, for a new
   * thread, the MCP servers to attach to it (ADR 0024).
   */
  target: Target;
  threads: ThreadsView;
  /** `lastSeq` of `GET /api/threads/{id}`: how far the conversation is; null until fetched. */
  threadLastSeq: number | null;
  notFound: boolean;
  onSendFailed: (message: string, status: number | undefined) => void;
  onSending: () => void;
  /** The mentions of the message in the box, asked for by the text of every message sent (ADR 0026). */
  mentions?: MentionsSource;
  /** The thread is read through a share link (ADR 0040): read-only, by the reader's own routes. Stable. */
  source?: ShareSource;
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
 * How long a stream that has delivered everything it has, and is still behind the thread's `lastSeq`,
 * is quiet before the page takes it as caught up. The log can end in events that have no frame and so
 * no resume point (`thread_shared`, `thread_unshared`, `ui_catalog`: docs/api/agui.md), which the
 * thread's `lastSeq` counts and the stream never reaches: a thread shared and then left alone would
 * otherwise never be "loaded", and its composer would wait for ever.
 */
export const QUIET_MS = 2_500;

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
  mentions,
  source,
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
        ...(mentions ? { mentions } : {}),
        ...(source ? { source } : {}),
        onSending: () => onSendingRef.current(),
        // The first send of the new-thread page creates the thread: go to it.
        onAccepted: ({ threadId: id }) => {
          if (threadId === null) router.push(`/threads/${id}`);
        },
      }),
    [threadId, router, mentions, source],
  );

  const snapshot = useSyncExternalStore(agent.onChange, agent.getSnapshot, agent.getSnapshot);

  const behind = threadLastSeq !== null && snapshot.lastSeq < threadLastSeq;
  const steady = useElapsedAt(
    behind && snapshot.connection === "open" && !snapshot.replaying && snapshot.openRun === null,
    snapshot.lastSeq,
    QUIET_MS,
  );
  // where the stream went quiet below the head: what it has is all there is (see `QUIET_MS`)
  const quiet = useRef<{ agent: ThreadAgent; at: number | null }>({ agent, at: null });
  if (quiet.current.agent !== agent) quiet.current = { agent, at: null };
  if (steady) quiet.current.at = snapshot.lastSeq;
  const caughtUp =
    (threadLastSeq !== null && snapshot.lastSeq >= threadLastSeq) ||
    (quiet.current.at !== null && snapshot.lastSeq <= quiet.current.at);
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
