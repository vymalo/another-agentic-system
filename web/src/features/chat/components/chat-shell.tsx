"use client";

import { AssistantRuntimeProvider } from "@assistant-ui/react";
import Link from "next/link";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { InlineStatus } from "@/components/inline-status";
import { AgentMenu } from "@/features/agents/components/agent-menu";
import {
  AgentsProblem,
  NewChatGreeting,
  Suggestions,
} from "@/features/agents/components/new-thread-panel";
import { useAgents } from "@/features/agents/hooks/use-agents";
import { effectiveSelection, requestedAgent } from "@/features/agents/lib/selection";
import { type Selection, useChatRuntime } from "@/features/chat/hooks/use-chat-runtime";
import { useThreadMeta } from "@/features/chat/hooks/use-thread";
import { parseJob } from "@/features/chat/lib/agui/vymalo";
import { ThreadPanel } from "@/features/panel/components/thread-panel";
import { PanelProvider } from "@/features/panel/hooks/use-panel";
import {
  SidebarOpeners,
  ThreadSidebar,
  ThreadsSheet,
} from "@/features/threads/components/thread-sidebar";
import { useThreads } from "@/features/threads/hooks/use-threads";
import { SIDEBAR_KEY } from "@/features/threads/lib/sidebar-state";
import { problemMessage } from "@/lib/api/client";
import { Composer } from "./composer";
import { DataUIs } from "./data-uis";
import { LiveDraftsProvider } from "./live-drafts";
import { LiveRuns } from "./live-runs";
import { SurfaceHostProvider } from "./surface/surface-host";
import { ThreadHeader } from "./thread-header";
import { ThreadViewProvider } from "./thread-view";

/**
 * Whether the desktop sidebar is open: remembered per browser, open when nothing is stored. A
 * script in the layout (`SIDEBAR_SCRIPT`, lib/sidebar-state.ts) puts `data-sidebar="closed"` on `<html>` before the first
 * paint and the stylesheet hides the sidebar by it, so a closed sidebar never flashes open while
 * this state, which starts open for the server's render, catches up.
 */
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
    if (next) delete document.documentElement.dataset.sidebar;
    else document.documentElement.dataset.sidebar = "closed";
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
  // the list also names the agent of an open thread and offers the others (the header's menu)
  const agents = useAgents(true);
  const [selection, setSelection] = useState<Selection>({ agentId: null, release: null });
  const [sendError, setSendError] = useState<string | null>(null);

  // Default to the agent a link asked for (`/?agent=reviewer`, "Start a new chat with …"), else
  // the first, so the composer works without an extra click.
  const loadedAgents = agents.agents;
  useEffect(() => {
    if (loadedAgents.length === 0) return;
    const first = loadedAgents[0]?.id ?? null;
    const asked = requestedAgent(loadedAgents, window.location.search) ?? first;
    setSelection((s) => (s.agentId === null ? { agentId: asked, release: null } : s));
  }, [loadedAgents]);

  const effective = useMemo<Selection>(
    () => effectiveSelection(agents.agents, selection),
    [agents.agents, selection],
  );

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
    setThreadsKey(`${threadId}:${state}:${meta.thread?.lastSeq}:${meta.thread?.title}`);
  }, [threadId, state, meta.thread?.lastSeq, meta.thread?.title]);

  const cancel = useCallback(() => {
    agent.cancel().catch((e: unknown) => setSendError(problemMessage(e)));
  }, [agent]);

  const [sidebarOpen, setSidebarOpen] = useSidebarOpen();
  // A toggle moves the focus to the control that now exists (the other one is gone or hidden).
  const collapseRef = useRef<HTMLButtonElement | null>(null);
  const openRef = useRef<HTMLButtonElement | null>(null);
  const focusAfterToggle = useRef<"open" | "collapse" | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs after each toggle, on purpose
  useEffect(() => {
    const target = focusAfterToggle.current === "open" ? openRef : collapseRef;
    if (!focusAfterToggle.current) return;
    focusAfterToggle.current = null;
    // a frame later: the button that was clicked is gone, and the browser's focus fix-up for a
    // removed element runs with the next rendering, which would undo a focus moved any earlier
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => target.current?.focus());
    });
    return () => cancelAnimationFrame(frame);
  }, [sidebarOpen]);
  const collapseSidebar = useCallback(() => {
    focusAfterToggle.current = "open";
    setSidebarOpen(false);
  }, [setSidebarOpen]);
  const openSidebar = useCallback(() => {
    focusAfterToggle.current = "collapse";
    setSidebarOpen(true);
  }, [setSidebarOpen]);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const thread = meta.thread;
  // a thread has a right-hand panel (its sources, later its activity); the new-chat page has none
  const Panels = threadId === null ? Fragment : PanelProvider;
  const composer = (
    <Composer
      state={state}
      job={job}
      failure={snapshot.failure}
      isNew={threadId === null}
      sendError={sendError}
      onCancel={cancel}
      inputRef={composerRef}
    />
  );
  const leading = (
    <>
      <ThreadsSheet threads={threads} />
      {sidebarOpen ? null : <SidebarOpeners onOpen={openSidebar} openRef={openRef} />}
    </>
  );
  const view = useMemo(
    () => ({
      state,
      waiting: snapshot.waiting,
      agentId: thread?.target.agentId ?? target.agentId,
      catalogVersion: snapshot.uiCatalog?.version,
    }),
    [state, snapshot.waiting, snapshot.uiCatalog?.version, thread?.target.agentId, target.agentId],
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
          <Panels>
            <div className="flex h-dvh overflow-hidden">
              <ThreadSidebar
                threads={threads}
                open={sidebarOpen}
                onCollapse={collapseSidebar}
                collapseRef={collapseRef}
              />
              <main className="flex min-h-0 min-w-0 flex-1 flex-col">
                {threadId === null ? (
                  <>
                    <header className="flex h-14 shrink-0 items-center gap-1 px-2 md:px-4">
                      {leading}
                      <AgentMenu
                        mode="new"
                        agents={agents}
                        value={effective}
                        onChange={setSelection}
                      />
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
                      agents={agents}
                      state={state}
                      waiting={snapshot.waiting}
                      connection={snapshot.connection}
                      leading={leading}
                      onRenamed={meta.apply}
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
                    <LiveDraftsProvider agent={agent}>
                      <Thread loading={!loaded} empty={loaded && snapshot.lastSeq === 0}>
                        {composer}
                      </Thread>
                    </LiveDraftsProvider>
                  </>
                )}
              </main>
              {threadId !== null && !(meta.notFound || snapshot.notFound) ? <ThreadPanel /> : null}
            </div>
          </Panels>
        </ThreadViewProvider>
      </SurfaceHostProvider>
    </AssistantRuntimeProvider>
  );
}
