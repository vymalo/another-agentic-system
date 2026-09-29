"use client";

import { AssistantRuntimeProvider } from "@assistant-ui/react";
import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { InlineStatus } from "@/components/inline-status";
import { NewThreadPanel } from "@/features/agents/components/new-thread-panel";
import { useAgents } from "@/features/agents/hooks/use-agents";
import { type Selection, useChatRuntime } from "@/features/chat/hooks/use-chat-runtime";
import { useThread } from "@/features/chat/hooks/use-thread";
import { ThreadSidebar, ThreadsSheet } from "@/features/threads/components/thread-sidebar";
import { useThreads } from "@/features/threads/hooks/use-threads";
import { Composer } from "./composer";
import { DataUIs } from "./data-uis";
import { ThreadHeader } from "./thread-header";

/** `null` is the new-thread page. Every navigation remounts, so no state leaks between threads. */
export function ChatShell({ threadId }: { threadId: string | null }) {
  const view = useThread(threadId);
  const threads = useThreads(`${threadId}:${view.state}:${view.thread?.lastSeq}`);
  const agents = useAgents(threadId === null);
  const [selection, setSelection] = useState<Selection>({ agentId: null, release: null });
  const [sendError, setSendError] = useState<string | null>(null);

  // Default to the first agent so the composer works without an extra click.
  const firstAgent = agents.agents[0]?.id ?? null;
  useEffect(() => {
    if (firstAgent && selection.agentId === null)
      setSelection({ agentId: firstAgent, release: null });
  }, [firstAgent, selection.agentId]);

  const effective = useMemo<Selection>(() => {
    const agent = agents.agents.find((a) => a.id === selection.agentId);
    if (!agent?.releases) return { agentId: selection.agentId, release: null };
    const r = agent.releases;
    const ok =
      selection.release !== null &&
      (selection.release in r.channels || (r.revisions ?? []).includes(selection.release));
    return { agentId: agent.id, release: ok ? selection.release : r.defaultChannel };
  }, [agents.agents, selection]);

  const runtime = useChatRuntime({
    threadId,
    view,
    threads,
    selection: effective,
    onSendError: setSendError,
  });

  const lastQuestion = useMemo(() => {
    for (let i = view.log.ordered.length - 1; i >= 0; i--) {
      const e = view.log.ordered[i];
      if (e?.kind === "agent_status" && e.data.status === "input_required") return e.data.detail;
    }
    return undefined;
  }, [view.log]);

  const composer = (
    <Composer
      state={view.state}
      isNew={threadId === null}
      lastQuestion={lastQuestion}
      sendError={sendError}
    />
  );

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <DataUIs />
      <div className="grid h-dvh grid-cols-1 md:grid-cols-[260px_minmax(0,1fr)]">
        <ThreadSidebar threads={threads} />
        <main className="flex min-h-0 min-w-0 flex-col px-3 md:px-4">
          <div className="pt-2 md:hidden">
            <ThreadsSheet threads={threads} />
          </div>
          {threadId === null ? (
            <>
              <div className="mx-auto w-full max-w-3xl">
                <NewThreadPanel agents={agents} selection={effective} onSelect={setSelection} />
              </div>
              <div className="mx-auto mt-auto w-full max-w-3xl">{composer}</div>
            </>
          ) : view.notFound ? (
            <div className="mx-auto w-full max-w-3xl pt-8">
              <InlineStatus role="status">
                Thread not found. <Link href="/">Start a new thread</Link>.
              </InlineStatus>
            </div>
          ) : (
            <>
              <div className="mx-auto w-full max-w-3xl">
                <ThreadHeader view={view} />
                {view.error ? (
                  <InlineStatus
                    tone="error"
                    role="alert"
                    action={{ label: "Retry", onClick: view.reload }}
                  >
                    Could not load the thread: {view.error}
                  </InlineStatus>
                ) : null}
              </div>
              <Thread>{composer}</Thread>
            </>
          )}
        </main>
      </div>
    </AssistantRuntimeProvider>
  );
}
