import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import type { Connection } from "@/features/chat/lib/agui/thread-agent";
import type { ApiThread, ThreadState } from "@/lib/api/types";
import { StateBadge } from "./state-badge";

type Props = { thread: ApiThread | null; state: ThreadState | undefined; connection: Connection };

export function ThreadHeader({ thread, state, connection }: Props) {
  const target = thread
    ? [thread.target.agentId, thread.target.release].filter(Boolean).join(" · ")
    : null;
  return (
    <header className="flex flex-wrap items-start justify-between gap-3 border-b pt-4 pb-3 md:flex-nowrap">
      <div className="min-w-0">
        {thread ? (
          <h1 className="text-lg leading-tight font-semibold [overflow-wrap:anywhere]">
            {thread.title}
          </h1>
        ) : (
          <>
            <h1 className="sr-only">Loading thread…</h1>
            <Skeleton aria-hidden="true" className="h-6 w-48" />
          </>
        )}
        {target ? <p className="mt-0.5 text-sm text-muted-foreground">{target}</p> : null}
      </div>
      <div className="flex shrink-0 items-center gap-3">
        {connection === "reconnecting" ? (
          <Badge variant="outline" role="status" className="text-muted-foreground">
            Reconnecting…
          </Badge>
        ) : null}
        <StateBadge state={state} />
      </div>
    </header>
  );
}
