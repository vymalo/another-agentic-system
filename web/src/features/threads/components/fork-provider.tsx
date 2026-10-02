"use client";

import { createContext, type ReactNode, useCallback, useContext, useMemo, useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import { type ForkTarget, useFork } from "@/features/threads/hooks/use-fork";
import { isActive, type ThreadState } from "@/lib/api/types";

/** What the chat needs to fork the open thread (ADR 0029); the defaults are those of a page with no thread. */
export type ThreadFork = {
  /** There is a thread to fork. False on the new-chat page. */
  available: boolean;
  /** The agent's turn is going on: nothing of it can be copied (the server says `turn_open`). */
  turnOpen: boolean;
  /** A fork is being made. */
  busy: boolean;
  /** Why the last fork was refused, in words (`TURN_OPEN_MESSAGE` for a turn that was not over). */
  error: string | null;
  /** What the refused request was: "fork" (from here, another agent) or "edit" (a message said again). */
  errorKind: "fork" | "edit";
  clearError: () => void;
  /** "Fork from here" on the turn that is the run `runId`; false when that run's end is not known (yet). */
  canForkRun: (runId: string | undefined) => boolean;
  forkRun: (runId: string) => void;
  /** Continue the conversation so far in a new chat with this agent (and release). */
  continueWith: (target: ForkTarget) => Promise<boolean>;
  /** The `seq` of the log event a person's message came in (the runtime's id of it); undefined until the page has read it. */
  seqOfMessage: (messageId: string) => number | undefined;
  /**
   * Says the message at `seq` again as `text`, in a new branch of the conversation (`replace`), and
   * goes to it. `messageId` is the id the new message gets: one per editing session, so that a retry
   * after a dropped connection is the same request. Resolves `false`, with `error`, when it was refused.
   */
  editMessage: (seq: number, text: string, messageId: string) => Promise<boolean>;
};

const NONE: ThreadFork = {
  available: false,
  turnOpen: false,
  busy: false,
  error: null,
  errorKind: "fork",
  clearError: () => {},
  canForkRun: () => false,
  forkRun: () => {},
  continueWith: async () => false,
  seqOfMessage: () => undefined,
  editMessage: async () => false,
};

const Context = createContext<ThreadFork>(NONE);

/** The fork actions of the open thread; the defaults (outside a thread) fork nothing. */
export const useThreadFork = (): ThreadFork => useContext(Context);

/**
 * Forking for the open thread: the turn actions, the agent menu's "continue with another agent"
 * and the edit of a message of the person (a branch). It never forks a turn that is not over (`turnOpen`: the thread is queued, working or
 * verifying), and says why the server refused a fork that was made anyway (a turn that began after
 * the page last heard, `turn_open`) in a line under the top bar.
 */
export function ForkProvider({
  threadId,
  agent,
  state,
  lastSeq,
  children,
}: {
  threadId: string;
  agent: ThreadAgent;
  state: ThreadState | undefined;
  /** The newest event the page has read: what the answers about the log (`seqOfMessage`) move with. */
  lastSeq: number;
  children: ReactNode;
}) {
  const forker = useFork(threadId);
  const { fork, busy, error, clearError } = forker;
  const [errorKind, setErrorKind] = useState<"fork" | "edit">("fork");
  const turnOpen = isActive(state);

  const canForkRun = useCallback(
    (runId: string | undefined) => runId !== undefined && agent.endOfRun(runId) !== undefined,
    [agent],
  );
  const forkRun = useCallback(
    (runId: string) => {
      const after = agent.endOfRun(runId);
      if (after !== undefined) void fork({ after });
    },
    [agent, fork],
  );
  const continueWith = useCallback(
    (target: ForkTarget) => {
      // the whole conversation: any event of the last turn names it
      const after = agent.getSnapshot().lastSeq;
      if (after <= 0) return Promise.resolve(false);
      setErrorKind("fork");
      return fork({ after, target });
    },
    [agent, fork],
  );
  const editMessage = useCallback(
    (seq: number, text: string, messageId: string) => {
      setErrorKind("edit");
      return fork({ replace: seq, text, messageId });
    },
    [fork],
  );
  // `lastSeq` is what makes the answer new: a message's `seq` is known once its group was read
  // biome-ignore lint/correctness/useExhaustiveDependencies: see above
  const seqOfMessage = useCallback((id: string) => agent.seqOfUser(id), [agent, lastSeq]);

  const value = useMemo<ThreadFork>(
    () => ({
      available: true,
      turnOpen,
      busy,
      error,
      errorKind,
      clearError,
      canForkRun,
      forkRun,
      continueWith,
      seqOfMessage,
      editMessage,
    }),
    [
      turnOpen,
      busy,
      error,
      errorKind,
      clearError,
      canForkRun,
      forkRun,
      continueWith,
      seqOfMessage,
      editMessage,
    ],
  );
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

/** Why the last fork was refused, under the top bar; nothing when there is nothing to say. */
export function ForkError() {
  const { error, errorKind, clearError } = useThreadFork();
  if (!error) return null;
  return (
    <div className="mx-auto w-full max-w-3xl px-4 md:px-6">
      <InlineStatus tone="error" role="alert" action={{ label: "Dismiss", onClick: clearError }}>
        {errorKind === "edit" ? "Could not edit the message" : "Could not fork the chat"}: {error}
      </InlineStatus>
    </div>
  );
}
