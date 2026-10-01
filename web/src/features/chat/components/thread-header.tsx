import type { ReactNode } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Skeleton } from "@/components/ui/skeleton";
import { useExportThread } from "@/features/chat/hooks/use-export-thread";
import type { Connection } from "@/features/chat/lib/agui/thread-agent";
import type { ApiThread, ThreadState } from "@/lib/api/types";
import { AgentPill } from "./agent-pill";
import { StateBadge } from "./state-badge";
import { ThreadMenu } from "./thread-menu";

type Props = {
  thread: ApiThread | null;
  state: ThreadState | undefined;
  connection: Connection;
  /** The agent asked something and waits for the answer: a blocked thread is then "Your turn". */
  waiting: boolean;
  /** The sidebar controls in front of the title (open the sidebar, the sheet on a phone). */
  leading?: ReactNode;
};

/** The top bar of a thread: its title, its agent, its state, and the overflow menu. */
export function ThreadHeader({ thread, state, connection, waiting, leading }: Props) {
  const exporter = useExportThread(thread?.id ?? null);
  return (
    <>
      <header className="flex h-14 shrink-0 items-center gap-2 px-2 md:px-4">
        {leading}
        <div className="flex min-w-0 flex-1 items-center gap-2.5 ps-1">
          {thread ? (
            <h1 className="min-w-0 truncate text-[0.9375rem] font-medium" title={thread.title}>
              {thread.title}
            </h1>
          ) : (
            <>
              <h1 className="sr-only">Loading thread…</h1>
              <Skeleton aria-hidden="true" className="h-5 w-48" />
            </>
          )}
          {thread ? (
            <AgentPill
              agentId={thread.target.agentId}
              release={thread.target.release}
              className="hidden lg:inline-flex"
            />
          ) : null}
        </div>
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
