import type { KeyboardEvent, ReactNode } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { AgentMenu } from "@/features/agents/components/agent-menu";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import { useExportThread } from "@/features/chat/hooks/use-export-thread";
import { type ThreadRenamer, useRenameThread } from "@/features/chat/hooks/use-rename-thread";
import type { Connection } from "@/features/chat/lib/agui/thread-agent";
import { PanelToggle } from "@/features/panel/components/panel-toggle";
import { ForkError, useThreadFork } from "@/features/threads/components/fork-provider";
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
  /** The server renamed the thread: here it is, so the title is not stale until the next fetch. */
  onRenamed: (thread: ApiThread) => void;
};

/** Why the other agents cannot be chosen while the agent works (the menu says it under the lists). */
const CONTINUE_BLOCKED =
  "The agent is working. Another agent, or another release, can continue this conversation once the turn is over.";

/** The title as a field while it is renamed: Enter or leaving it saves, Escape gives it up. */
function TitleField({ renamer, current }: { renamer: ThreadRenamer; current: string }) {
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      renamer.save();
    } else if (event.key === "Escape") {
      event.preventDefault();
      renamer.cancel();
    }
  };
  return (
    <>
      {/* the page keeps its heading while the field is open */}
      <h1 className="sr-only">{current}</h1>
      <Input
        ref={renamer.field}
        value={renamer.draft ?? ""}
        onChange={(event) => renamer.change(event.target.value)}
        onKeyDown={onKeyDown}
        onBlur={renamer.save}
        maxLength={200}
        disabled={renamer.saving}
        aria-label="Thread title"
        aria-invalid={renamer.error !== null}
        className="h-8 max-w-md text-[0.9375rem]"
      />
    </>
  );
}

/**
 * The top bar of a thread: the agent picker (a menu, like a model picker), the title, the state,
 * and the overflow menu. The title is the page's heading; below `md` it is for screen readers only.
 */
export function ThreadHeader({
  thread,
  agents,
  state,
  connection,
  waiting,
  leading,
  onRenamed,
}: Props) {
  const exporter = useExportThread(thread?.id ?? null);
  const renamer = useRenameThread(thread, onRenamed);
  const fork = useThreadFork();
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
          onContinue={(to) =>
            to.agentId === null
              ? Promise.resolve(false)
              : fork.continueWith({ agentId: to.agentId, release: to.release })
          }
          {...(fork.turnOpen ? { blocked: CONTINUE_BLOCKED } : {})}
        />
        <div className="flex min-w-0 flex-1 items-center ps-2">
          {thread && renamer.draft !== null ? (
            <TitleField renamer={renamer} current={thread.title} />
          ) : thread ? (
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
          <div className="flex items-center">
            <PanelToggle />
            <ThreadMenu exporter={exporter} renamer={renamer} disabled={thread === null} />
          </div>
        </div>
      </header>
      {renamer.error ? (
        <div className="mx-auto w-full max-w-3xl px-4 md:px-6">
          <InlineStatus tone="error" role="alert">
            Could not rename the thread: {renamer.error}
          </InlineStatus>
        </div>
      ) : null}
      <ForkError />
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
