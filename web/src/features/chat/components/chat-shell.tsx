"use client";

import { AssistantRuntimeProvider } from "@assistant-ui/react";
import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { NewThreadPanel } from "@/features/agents/components/new-thread-panel";
import { useAgents } from "@/features/agents/hooks/use-agents";
import { type Selection, useChatRuntime } from "@/features/chat/hooks/use-chat-runtime";
import { useThread } from "@/features/chat/hooks/use-thread";
import { ThreadList } from "@/features/threads/components/thread-list";
import { useThreads } from "@/features/threads/hooks/use-threads";
import { Composer } from "./composer";
import { Thread } from "./thread";
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

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <div className="shell">
        <ThreadList threads={threads} />
        <main className="main">
          {threadId === null ? (
            <NewThreadPanel agents={agents} selection={effective} onSelect={setSelection} />
          ) : view.notFound ? (
            <div className="main__message">
              <InlineStatus role="status">
                Thread not found. <Link href="/">Start a new thread</Link>.
              </InlineStatus>
            </div>
          ) : (
            <>
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
              <Thread empty={view.log.ordered.length === 0} />
            </>
          )}
          {threadId !== null && view.notFound ? null : (
            <Composer
              state={view.state}
              isNew={threadId === null}
              lastQuestion={lastQuestion}
              sendError={sendError}
            />
          )}
        </main>
      </div>
    </AssistantRuntimeProvider>
  );
}
