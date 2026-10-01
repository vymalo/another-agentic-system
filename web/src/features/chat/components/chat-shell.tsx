"use client";

import { AssistantRuntimeProvider } from "@assistant-ui/react";
import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { InlineStatus } from "@/components/inline-status";
import {
  AgentPicker,
  AgentsProblem,
  NewChatGreeting,
  Suggestions,
} from "@/features/agents/components/new-thread-panel";
import { useAgents } from "@/features/agents/hooks/use-agents";
import { type Selection, useChatRuntime } from "@/features/chat/hooks/use-chat-runtime";
import { useThreadMeta } from "@/features/chat/hooks/use-thread";
import { parseJob } from "@/features/chat/lib/agui/vymalo";
import {
  SidebarOpeners,
  ThreadSidebar,
  ThreadsSheet,
} from "@/features/threads/components/thread-sidebar";
import { useThreads } from "@/features/threads/hooks/use-threads";
import { problemMessage } from "@/lib/api/client";
import { AgentPill } from "./agent-pill";
import { Composer } from "./composer";
import { DataUIs } from "./data-uis";
import { LiveRuns } from "./live-runs";
import { SurfaceHostProvider } from "./surface/surface-host";
import { ThreadHeader } from "./thread-header";
import { ThreadViewProvider } from "./thread-view";

const SIDEBAR_KEY = "chat.sidebar";

/** Whether the desktop sidebar is open: remembered per browser, open when nothing is stored. */
function useSidebarOpen(): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(true);
  useEffect(() => {
    try {
      if (window.localStorage.getItem(SIDEBAR_KEY) === "closed") setOpen(false);
    } catch {
      // storage may be blocked: the sidebar stays open
    }
  }, []);
  const set = useCallback((next: boolean) => {
    setOpen(next);
    try {
      window.localStorage.setItem(SIDEBAR_KEY, next ? "open" : "closed");
    } catch {
      // not remembered, still toggled
    }
  }, []);
  return [open, set];
}

/** `null` is the new-thread page. Every navigation remounts, so no state leaks between threads. */
export function ChatShell({ threadId }: { threadId: string | null }) {
  const meta = useThreadMeta(threadId);
  const [threadsKey, setThreadsKey] = useState("");
  const threads = useThreads(threadsKey);
  const agents = useAgents(threadId === null);
  const [selection, setSelection] = useState<Selection>({ agentId: null, release: null });
  const [sendError, setSendError] = useState<string | null>(null);

  // Default to the first agent so the composer works without an extra click.
  const firstAgent = agents.agents[0]?.id ?? null;
  useEffect(() => {
    if (firstAgent) {
      setSelection((s) => (s.agentId === null ? { agentId: firstAgent, release: null } : s));
    }
  }, [firstAgent]);

  const effective = useMemo<Selection>(() => {
    const agent = agents.agents.find((a) => a.id === selection.agentId);
    if (!agent?.releases) return { agentId: selection.agentId, release: null };
    const r = agent.releases;
    const ok =
      selection.release !== null &&
      (selection.release in r.channels || (r.revisions ?? []).includes(selection.release));
    return { agentId: agent.id, release: ok ? selection.release : r.defaultChannel };
  }, [agents.agents, selection]);

  // A send goes to the thread's own agent; only a new thread takes the picker's choice.
  const target = useMemo<Selection>(
    () =>
      threadId === null
        ? effective
        : { agentId: meta.thread?.target.agentId ?? null, release: null },
    [threadId, effective, meta.thread?.target.agentId],
  );

  const onSendFailed = useCallback((message: string) => setSendError(message), []);
  const onSending = useCallback(() => setSendError(null), []);
  const chat = useChatRuntime({
    threadId,
    target,
    threads,
    threadLastSeq: meta.thread?.lastSeq ?? null,
    notFound: meta.notFound,
    onSendFailed,
    onSending,
  });
  const { snapshot, agent, runtime, loaded } = chat;

  // What the server says the thread is doing: the stream's newest snapshot, else the fetch.
  const state = snapshot.state ?? meta.thread?.state;
  // Where the job stands under a verification gate (ADR 0018): the stream's newest snapshot says
  // so, else the fetch; a thread without a gate has none.
  const job = snapshot.state !== undefined ? snapshot.job : parseJob(meta.thread?.job);

  // The conversation moved on: the title and lastSeq of the resource move with it.
  const { refetchSoon } = meta;
  useEffect(() => {
    if (snapshot.lastSeq > 0) refetchSoon();
  }, [snapshot.lastSeq, refetchSoon]);
  useEffect(() => {
    setThreadsKey(`${threadId}:${state}:${meta.thread?.lastSeq}`);
  }, [threadId, state, meta.thread?.lastSeq]);

  const cancel = useCallback(() => {
    agent.cancel().catch((e: unknown) => setSendError(problemMessage(e)));
  }, [agent]);

  const [sidebarOpen, setSidebarOpen] = useSidebarOpen();
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const thread = meta.thread;
  const composer = (
    <Composer
      state={state}
      job={job}
      failure={snapshot.failure}
      isNew={threadId === null}
      sendError={sendError}
      onCancel={cancel}
      inputRef={composerRef}
      toolbar={
        threadId === null ? (
          <AgentPicker agents={agents} selection={effective} onSelect={setSelection} />
        ) : thread ? (
          <AgentPill agentId={thread.target.agentId} release={thread.target.release} />
        ) : null
      }
    />
  );
  const leading = (
    <>
      <ThreadsSheet threads={threads} />
      {sidebarOpen ? null : <SidebarOpeners onOpen={() => setSidebarOpen(true)} />}
    </>
  );
  const view = useMemo(
    () => ({
      state,
      waiting: snapshot.waiting,
      agentId: thread?.target.agentId ?? target.agentId,
    }),
    [state, snapshot.waiting, thread?.target.agentId, target.agentId],
  );

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <SurfaceHostProvider
        agent={agent}
        state={state}
        composerRef={composerRef}
        onRejected={onSendFailed}
      >
        <DataUIs />
        <LiveRuns agent={agent} runtime={runtime} />
        <ThreadViewProvider value={view}>
          <div className="flex h-dvh overflow-hidden">
            <ThreadSidebar
              threads={threads}
              open={sidebarOpen}
              onCollapse={() => setSidebarOpen(false)}
            />
            <main className="flex min-h-0 min-w-0 flex-1 flex-col">
              {threadId === null ? (
                <>
                  <header className="flex h-14 shrink-0 items-center gap-2 px-2 md:px-4">
                    {leading}
                  </header>
                  <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
                    <div className="mx-auto flex w-full max-w-3xl flex-1 flex-col justify-center gap-7 px-4 pt-4 pb-[12vh] md:px-6">
                      <NewChatGreeting agents={agents} selection={effective} />
                      <AgentsProblem agents={agents} />
                      {composer}
                      <Suggestions inputRef={composerRef} />
                    </div>
                  </div>
                </>
              ) : meta.notFound || snapshot.notFound ? (
                <>
                  <header className="flex h-14 shrink-0 items-center gap-2 px-2 md:px-4">
                    {leading}
                  </header>
                  <div className="mx-auto w-full max-w-3xl px-4 pt-8 md:px-6">
                    <InlineStatus role="status">
                      Thread not found. <Link href="/">Start a new thread</Link>.
                    </InlineStatus>
                  </div>
                </>
              ) : (
                <>
                  <ThreadHeader
                    thread={thread}
                    state={state}
                    waiting={snapshot.waiting}
                    connection={snapshot.connection}
                    leading={leading}
                  />
                  {meta.error ? (
                    <div className="mx-auto w-full max-w-3xl px-4 md:px-6">
                      <InlineStatus
                        tone="error"
                        role="alert"
                        action={{ label: "Retry", onClick: meta.reload }}
                      >
                        Could not load the thread: {meta.error}
                      </InlineStatus>
                    </div>
                  ) : null}
                  <Thread loading={!loaded} empty={loaded && snapshot.lastSeq === 0}>
                    {composer}
                  </Thread>
                </>
              )}
            </main>
          </div>
        </ThreadViewProvider>
      </SurfaceHostProvider>
    </AssistantRuntimeProvider>
  );
}
