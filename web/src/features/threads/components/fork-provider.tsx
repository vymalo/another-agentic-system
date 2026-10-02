"use client";

import { createContext, type ReactNode, useCallback, useContext, useMemo } from "react";
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
  clearError: () => void;
  /** "Fork from here" on the turn that is the run `runId`; false when that run's end is not known (yet). */
  canForkRun: (runId: string | undefined) => boolean;
  forkRun: (runId: string) => void;
  /** Continue the conversation so far in a new chat with this agent (and release). */
  continueWith: (target: ForkTarget) => Promise<boolean>;
};

const NONE: ThreadFork = {
  available: false,
  turnOpen: false,
  busy: false,
  error: null,
  clearError: () => {},
  canForkRun: () => false,
  forkRun: () => {},
  continueWith: async () => false,
};

const Context = createContext<ThreadFork>(NONE);

/** The fork actions of the open thread; the defaults (outside a thread) fork nothing. */
export const useThreadFork = (): ThreadFork => useContext(Context);

/**
 * Forking for the open thread: the turn actions, and the agent menu's "continue with another
 * agent". It never forks a turn that is not over (`turnOpen`: the thread is queued, working or
 * verifying), and says why the server refused a fork that was made anyway (a turn that began after
 * the page last heard, `turn_open`) in a line under the top bar.
 */
export function ForkProvider({
  threadId,
  agent,
  state,
  children,
}: {
  threadId: string;
  agent: ThreadAgent;
  state: ThreadState | undefined;
  children: ReactNode;
}) {
  const forker = useFork(threadId);
  const { fork, busy, error, clearError } = forker;
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
      return after > 0 ? fork({ after, target }) : Promise.resolve(false);
    },
    [agent, fork],
  );

  const value = useMemo<ThreadFork>(
    () => ({
      available: true,
      turnOpen,
      busy,
      error,
      clearError,
      canForkRun,
      forkRun,
      continueWith,
    }),
    [turnOpen, busy, error, clearError, canForkRun, forkRun, continueWith],
  );
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

/** Why the last fork was refused, under the top bar; nothing when there is nothing to say. */
export function ForkError() {
  const { error, clearError } = useThreadFork();
  if (!error) return null;
  return (
    <div className="mx-auto w-full max-w-3xl px-4 md:px-6">
      <InlineStatus tone="error" role="alert" action={{ label: "Dismiss", onClick: clearError }}>
        Could not fork the chat: {error}
      </InlineStatus>
    </div>
  );
}
