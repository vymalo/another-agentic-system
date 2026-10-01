import type { ReactNode } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Skeleton } from "@/components/ui/skeleton";
import { AgentMenu } from "@/features/agents/components/agent-menu";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import { useExportThread } from "@/features/chat/hooks/use-export-thread";
import type { Connection } from "@/features/chat/lib/agui/thread-agent";
import type { ApiThread, ThreadState } from "@/lib/api/types";
import { StateBadge } from "./state-badge";
import { ThreadMenu } from "./thread-menu";

type Props = {
  thread: ApiThread | null;
  /** The agents, for the picker: it names the thread's agent and offers the others as a new chat. */
  agents: AgentsView;
  state: ThreadState | undefined;
  connection: Connection;
  /** The agent asked something and waits for the answer: a blocked thread is then "Your turn". */
  waiting: boolean;
  /** The sidebar controls in front of the picker (open the sidebar, the sheet on a phone). */
  leading?: ReactNode;
};

/**
 * The top bar of a thread: the agent picker (a menu, like a model picker), the title, the state,
 * and the overflow menu. The title is the page's heading; below `md` it is for screen readers only.
 */
export function ThreadHeader({ thread, agents, state, connection, waiting, leading }: Props) {
  const exporter = useExportThread(thread?.id ?? null);
  return (
    <>
      <header className="flex h-14 shrink-0 items-center gap-1 px-2 md:px-4">
        {leading}
        <AgentMenu
          mode="thread"
          agents={agents}
          value={{
            agentId: thread?.target.agentId ?? null,
            release: thread?.target.release ?? null,
          }}
        />
        <div className="flex min-w-0 flex-1 items-center ps-2">
          {thread ? (
            <h1
              className="min-w-0 truncate text-[0.9375rem] text-muted-foreground max-md:sr-only"
              title={thread.title}
            >
              {thread.title}
            </h1>
          ) : (
            <>
              <h1 className="sr-only">Loading thread…</h1>
              <Skeleton aria-hidden="true" className="h-5 w-48 max-md:hidden" />
            </>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {connection === "reconnecting" ? (
            <span
              role="status"
              className="inline-flex h-7 items-center rounded-full bg-muted px-2.5 text-[0.8125rem] text-muted-foreground"
            >
              Reconnecting…
            </span>
          ) : null}
          <StateBadge state={state} needsAnswer={waiting} />
          <ThreadMenu exporter={exporter} disabled={thread === null} />
        </div>
      </header>
      {exporter.error ? (
        <div className="mx-auto w-full max-w-3xl px-4 md:px-6">
          <InlineStatus tone="error" role="alert">
            Could not export the thread: {exporter.error}
          </InlineStatus>
        </div>
      ) : null}
    </>
  );
}
