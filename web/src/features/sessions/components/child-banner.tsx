"use client";

import { BotIcon, Loader2Icon } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ApiThread } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { useChildView } from "../hooks/use-child-view";
import { subagentStateWords } from "../lib/words";
import { ParentLink } from "./parent-link";
import { useSessions } from "./sessions-provider";

/**
 * The strip under a subagent's top bar (child-sessions/v1): the way back to the parent chat, the helper
 * that works here, whether it stays open or closes when done, where it stands in words, and Close
 * (the dialog of the Subagents tab). It reads `childOf` of the resource and the live `session-self`
 * card, the newer winning. The helper's name is the agent's text, drawn as text.
 */
export function ChildBanner({
  thread,
  threadId,
  rows,
}: {
  thread: ApiThread | null;
  threadId: string;
  rows: readonly ApiThread[];
}) {
  const view = useChildView(thread, threadId);
  const sessions = useSessions();
  if (view === null) return null;
  const name = view.agent || "subagent";
  const closing = view.phase === "closing";
  // a close that waits for the round to end can be made stronger ("Stop and close"); a closed one, or
  // one that was asked to stop, cannot
  const canClose = sessions?.closeOffered(view) ?? false;
  return (
    <div className="mx-auto w-full max-w-3xl px-4 pb-2 md:px-6">
      <div
        data-slot="child-banner"
        data-phase={view.phase}
        className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 rounded-xl border bg-card px-3 py-2 text-xs text-muted-foreground"
      >
        <ParentLink parent={view.parent} rows={rows} className="text-[0.8125rem]" />
        <span className="inline-flex min-w-0 items-center gap-1.5">
          <BotIcon aria-hidden="true" className="size-3.5 shrink-0" />
          <span data-slot="child-agent" className="truncate font-medium text-foreground">
            {name}
          </span>
        </span>
        <span
          data-slot="child-keep"
          className="rounded-full bg-muted px-2 py-0.5 text-[0.6875rem] text-muted-foreground"
        >
          {view.keepOpen ? "Stays open" : "Closes when done"}
        </span>
        <span role="status" className="inline-flex items-center gap-1.5">
          {closing ? (
            <Loader2Icon aria-hidden="true" className="size-3 motion-safe:animate-spin" />
          ) : (
            <span
              aria-hidden="true"
              className={cn(
                "size-1.5 rounded-full bg-muted-foreground/60",
                view.state === "working" &&
                  view.report !== "blocked" &&
                  "bg-brand motion-safe:animate-pulse",
                view.report === "blocked" && view.state !== "closed" && "bg-warning",
              )}
            />
          )}
          <span data-slot="child-state">{subagentStateWords(view)}</span>
        </span>
        {canClose ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="ms-auto h-7"
            onClick={(e) => sessions?.askClose(view, e.currentTarget)}
          >
            Close <span className="sr-only">{name}</span>
          </Button>
        ) : null}
      </div>
    </div>
  );
}
