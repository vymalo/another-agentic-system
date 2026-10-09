"use client";

import { AssistantRuntimeProvider } from "@assistant-ui/react";
import { RotateCcwIcon } from "lucide-react";
import Link from "next/link";
import { Fragment, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { InlineStatus } from "@/components/inline-status";
import { AgentMenu } from "@/features/agents/components/agent-menu";
import { AgentNamesProvider } from "@/features/agents/components/agent-names-context";
import {
  AgentsProblem,
  NewChatGreeting,
  Suggestions,
} from "@/features/agents/components/new-thread-panel";
import { RegistryNotice } from "@/features/agents/components/registry-notice";
import { useAgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { useAgents } from "@/features/agents/hooks/use-agents";
import { agentNamed, effectiveSelection, requestedAgent } from "@/features/agents/lib/selection";
import { type Selection, useChatRuntime } from "@/features/chat/hooks/use-chat-runtime";
import { useThreadMeta } from "@/features/chat/hooks/use-thread";
import type { Target } from "@/features/chat/lib/agui/thread-agent";
import { parseJob } from "@/features/chat/lib/agui/vymalo";
import { STEER_URI } from "@/features/chat/lib/send";
import { NoAccess } from "@/features/me/components/no-access";
import { ReadOnlyNotice } from "@/features/me/components/read-only-notice";
import { useMe } from "@/features/me/hooks/use-me";
import {
  hasNoAccess,
  invokable,
  newChatAccess,
  scopeOf,
  threadAccess,
} from "@/features/me/lib/access";
import { MentionsProvider } from "@/features/mentions/components/mentioned-text";
import { MentionsWarning } from "@/features/mentions/components/mentions-warning";
import { useBoxMentions } from "@/features/mentions/hooks/use-mentions";
import { MentionsStore } from "@/features/mentions/lib/store";
import { ThreadPanel } from "@/features/panel/components/thread-panel";
import { PanelProvider } from "@/features/panel/hooks/use-panel";
import { useKeepSessionWarm } from "@/features/session/hooks/use-keep-session-warm";
import { BranchesProvider } from "@/features/threads/components/branches-provider";
import { ForkProvider } from "@/features/threads/components/fork-provider";
import {
  SidebarOpeners,
  ThreadSidebar,
  ThreadsSheet,
} from "@/features/threads/components/thread-sidebar";
import { useScrollToMessage } from "@/features/threads/hooks/use-scroll-to-message";
import { useThreads } from "@/features/threads/hooks/use-threads";
import { SIDEBAR_KEY } from "@/features/threads/lib/sidebar-state";
import { ToolServersProvider } from "@/features/tools/components/tool-servers-context";
import { ToolsPicker, ToolsWarning } from "@/features/tools/components/tools-picker";
import { useThreadTools } from "@/features/tools/hooks/use-thread-tools";
import { useToolServers } from "@/features/tools/hooks/use-tool-servers";
import { sameSet, stillOffered } from "@/features/tools/lib/servers";
import { problemMessage } from "@/lib/api/client";
import { Composer } from "./composer";
import { DataUIs } from "./data-uis";
import { DeliveryProvider } from "./delivery-note";
import { LiveDraftsProvider } from "./live-drafts";
import { LiveRuns } from "./live-runs";
import { SurfaceHostProvider } from "./surface/surface-host";
import { ThreadHeader } from "./thread-header";
import { ThreadViewProvider } from "./thread-view";
import { UsageRing } from "./usage-ring";

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

/**
 * `null` is the new-thread page. Every navigation remounts, so no state leaks between threads.
 *
 * Who the person is (`GET /api/me`, ADR 0033) comes first: a person whose roles grant nothing gets
 * the screen that says so, and the chat (which would only list 403s) is not mounted. Everything
 * else the roles decide, the chat hides or disables itself; the orchestrator enforces it either way.
 */
export function ChatShell({ threadId }: { threadId: string | null }) {
  const { me } = useMe();
  if (me && hasNoAccess(me)) return <NoAccess me={me} />;
  return <Chat threadId={threadId} />;
}

function Chat({ threadId }: { threadId: string | null }) {
  useKeepSessionWarm();
  const { me, status: meStatus } = useMe();
  const meta = useThreadMeta(threadId);
  const [threadsKey, setThreadsKey] = useState("");
  const threads = useThreads(threadsKey);
  // the list also names the agent of an open thread and offers the others (the header's menu); only the
  // ones `agent.invoke` covers are offered, and the thread's own agent is named whatever the roles say
  const listed = useAgents(true);
  const threadAgentId = meta.thread?.target.agentId ?? null;
  const invokableAgents = useMemo(
    () => invokable(listed.agents, me, threadAgentId),
    [listed.agents, me, threadAgentId],
  );
  const agents = { ...listed, agents: invokableAgents };
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

  // The MCP servers (ADR 0024). The deployment's list is read for the chat (the picker, the names and
  // icons of the steps); a person whose roles hold no `thread.write` is not offered it (it would be a 403).
  // (asked once who the person is is known; an orchestrator that cannot say is asked, and decides)
  const mayAttach = me ? scopeOf(me, "thread.write") !== null : meStatus === "unknown";
  const toolServers = useToolServers(mayAttach);
  // a new chat keeps its choice here until the run that creates the thread carries it
  const [newTools, setNewTools] = useState<string[]>([]);
  const { servers: offered } = toolServers;
  const chosenAgent = effective.agentId;
  useEffect(() => {
    // another agent, or a list that no longer has a server: what the orchestrator would refuse (422) goes
    setNewTools((chosen) => {
      const kept = stillOffered(chosen, offered, chosenAgent);
      return sameSet(kept, chosen) ? chosen : kept;
    });
  }, [offered, chosenAgent]);

  // A send goes to the thread's own agent; only a new thread takes the picker's choice.
  const target = useMemo<Target>(
    () =>
      threadId === null
        ? { ...effective, tools: newTools }
        : { agentId: meta.thread?.target.agentId ?? null, release: null },
    [threadId, effective, newTools, meta.thread?.target.agentId],
  );

  // the mentions of the message in the box (ADR 0026), asked for by the agent when it sends it
  const [mentionsStore] = useState(() => new MentionsStore());
  const retryAgents = listed.retry;
  const onSendFailed = useCallback(
    (message: string, status?: number) => {
      setSendError(message);
      // an agent that moved or went (422), a registry that did not answer (503): read the list again,
      // so that the next send is checked against what is there now
      if (status === 422 || status === 503) retryAgents();
    },
    [retryAgents],
  );
  const onSending = useCallback(() => setSendError(null), []);
  const chat = useChatRuntime({
    threadId,
    target,
    threads,
    threadLastSeq: meta.thread?.lastSeq ?? null,
    notFound: meta.notFound,
    onSendFailed,
    onSending,
    mentions: mentionsStore,
  });
  const { snapshot, agent, runtime, loaded, revealed } = chat;

  // What the server says the thread is doing: the stream's newest snapshot, else the fetch.
  const state = snapshot.state ?? meta.thread?.state;
  // Where the job stands under a verification gate (ADR 0018): the stream's newest snapshot says
  // so, else the fetch; a thread without a gate has none.
  const job = snapshot.state !== undefined ? snapshot.job : parseJob(meta.thread?.job);

  // The conversation moved on: the title, the description and lastSeq of the resource move with it.
  const { refetchSoon } = meta;
  useEffect(() => {
    if (snapshot.lastSeq > 0) refetchSoon();
  }, [snapshot.lastSeq, refetchSoon]);
  useEffect(() => {
    setThreadsKey(
      `${threadId}:${state}:${meta.thread?.lastSeq}:${meta.thread?.title}:${meta.thread?.description}:${meta.thread?.share?.effective}`,
    );
  }, [
    threadId,
    state,
    meta.thread?.lastSeq,
    meta.thread?.title,
    meta.thread?.description,
    meta.thread?.share?.effective,
  ]);

  // The servers attached to an open thread: the log's truth (the stream's snapshot, else the resource)
  // and the person's own change in between; a new chat has the choice it is keeping.
  const attached = useThreadTools({
    threadId,
    stream: snapshot.tools,
    streamSeq: snapshot.lastSeq,
    fetched: meta.thread?.tools,
    fetchedSeq: meta.thread?.lastSeq ?? null,
    refetchSoon,
  });
  const chosenTools = threadId === null ? newTools : attached.tools;
  // the agent's card, read live: does it list thread-tools/v1? Said before anything is sent.
  const toolsAgentId = target.agentId;
  const capabilities = useAgentCapabilities(toolsAgentId);
  const toolsAgentName =
    (toolsAgentId ? agentNamed(listed.agents, toolsAgentId) : undefined)?.name ??
    toolsAgentId ??
    "The agent";

  // does the agent read a message at its next step (steer/v1), or after its turn? Its card says.
  const steers = capabilities.supports(STEER_URI);

  // the agents that may be mentioned: the ones the roles let the person invoke, but not the agent that
  // reads the message (an agent cannot be mentioned in its own thread, the orchestrator says 422)
  const mentionable = useMemo(
    () => invokableAgents.filter((a) => a.id !== toolsAgentId),
    [invokableAgents, toolsAgentId],
  );
  // a card that moved since a mention was picked: the mentions in the box follow the list
  const { agents: listedAgents } = listed;
  useEffect(() => {
    mentionsStore.followList(listedAgents);
  }, [mentionsStore, listedAgents]);
  const boxMentions = useBoxMentions(mentionsStore);

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
  // what the roles let the person do here: a thread of another's, or one of an agent they may not use,
  // is read-only (nothing is offered that the orchestrator would answer with a 403)
  const access = threadId === null ? newChatAccess(me) : threadAccess(me, thread);
  const readOnly = access.readOnly ? access.reason : null;
  // a thread has a right-hand panel (its sources, later its activity); the new-chat page has none
  const Panels = threadId === null ? Fragment : PanelProvider;
  const composer = readOnly ? (
    <ReadOnlyNotice reason={readOnly} isNew={threadId === null} />
  ) : (
    <Composer
      state={state}
      job={job}
      failure={snapshot.failure}
      isNew={threadId === null}
      sendError={sendError}
      onCancel={cancel}
      sending={{
        send: (text, mode) => agent.sendWhileWorking(text, mode),
        onFailed: onSendFailed,
        agent: toolsAgentName,
        steers,
        // the replay has to be on screen: a send before it replaces the turns it has not drawn
        ready: threadId === null || (loaded && !snapshot.replaying),
      }}
      inputRef={composerRef}
      usage={<UsageRing usage={snapshot.usage} />}
      mentions={{ agents: mentionable, store: mentionsStore }}
      toolbar={
        <ToolsPicker
          view={toolServers}
          agentId={toolsAgentId}
          chosen={chosenTools}
          onChange={threadId === null ? setNewTools : (next) => void attached.set(next)}
          busy={attached.busy}
          capabilities={capabilities}
        />
      }
      notices={
        <>
          <ToolsWarning
            chosen={chosenTools}
            capabilities={capabilities}
            agentName={toolsAgentName}
          />
          <MentionsWarning
            mentioned={boxMentions.length}
            capabilities={capabilities}
            agentName={toolsAgentName}
          />
          {attached.error ? (
            <InlineStatus tone="error" role="alert">
              {attached.error}
            </InlineStatus>
          ) : null}
        </>
      }
    />
  );
  const leading = (
    <>
      <ThreadsSheet threads={threads} />
      {sidebarOpen ? null : <SidebarOpeners onOpen={openSidebar} openRef={openRef} />}
    </>
  );
  // a version's link (`#m-<seq>`) scrolls to the message once the conversation has it
  useScrollToMessage(threadId, revealed);
  // a thread can be forked (ADR 0029), from its turns and from the agent menu, and a message of
  // the person edited into a branch whose versions are picked between; a new chat has none of it
  const inFork = (children: ReactNode) =>
    threadId === null ? (
      children
    ) : (
      <ForkProvider
        threadId={threadId}
        agent={agent}
        state={state}
        lastSeq={snapshot.lastSeq}
        readOnly={readOnly}
      >
        <BranchesProvider threadId={threadId} refreshKey={state ?? ""}>
          {children}
        </BranchesProvider>
      </ForkProvider>
    );
  const view = useMemo(
    () => ({
      state,
      waiting: snapshot.waiting,
      agentId: thread?.target.agentId ?? target.agentId,
      catalogVersion: snapshot.uiCatalog?.version,
      forkedFrom: thread?.forkedFrom,
    }),
    [
      state,
      snapshot.waiting,
      snapshot.uiCatalog?.version,
      thread?.target.agentId,
      target.agentId,
      thread?.forkedFrom,
    ],
  );

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ToolServersProvider servers={toolServers.servers}>
        <AgentNamesProvider agents={listed.agents}>
          <MentionsProvider store={mentionsStore} agents={listed.agents}>
            <SurfaceHostProvider
              agent={agent}
              readOnly={readOnly}
              state={state}
              composerRef={composerRef}
              onRejected={onSendFailed}
            >
              <DataUIs />
              <LiveRuns agent={agent} runtime={runtime} />
              <ThreadViewProvider value={view}>
                {inFork(
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
                                {readOnly ? null : <AgentsProblem agents={agents} />}
                                <RegistryNotice agents={agents} />
                                {composer}
                                {readOnly ? null : <Suggestions inputRef={composerRef} />}
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
                                  action={{
                                    label: "Retry",
                                    icon: RotateCcwIcon,
                                    onClick: meta.reload,
                                  }}
                                >
                                  Could not load the thread: {meta.error}
                                </InlineStatus>
                              </div>
                            ) : null}
                            <DeliveryProvider agent={toolsAgentName} steers={steers}>
                              <LiveDraftsProvider agent={agent}>
                                <Thread
                                  loading={!revealed}
                                  empty={loaded && snapshot.lastSeq === 0}
                                >
                                  {composer}
                                </Thread>
                              </LiveDraftsProvider>
                            </DeliveryProvider>
                          </>
                        )}
                      </main>
                      {threadId !== null && !(meta.notFound || snapshot.notFound) ? (
                        <ThreadPanel />
                      ) : null}
                    </div>
                  </Panels>,
                )}
              </ThreadViewProvider>
            </SurfaceHostProvider>
          </MentionsProvider>
        </AgentNamesProvider>
      </ToolServersProvider>
    </AssistantRuntimeProvider>
  );
}
