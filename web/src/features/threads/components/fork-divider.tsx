"use client";

import { MessagePrimitive } from "@assistant-ui/react";
import { GitForkIcon } from "lucide-react";
import Link from "next/link";
import { useEffect, useState } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import type { ForkContent } from "@/features/chat/lib/agui/vymalo";
import { api } from "@/lib/api/client";

/** The agent of the thread a fork was made from; null until it is read, and when it cannot be. */
function useParentAgent(parentId: string | undefined): string | null {
  const [agentId, setAgentId] = useState<string | null>(null);
  useEffect(() => {
    if (parentId === undefined) return;
    let current = true;
    api
      .GET("/api/threads/{threadId}", { params: { path: { threadId: parentId } } })
      .then(({ data }) => {
        if (current && data) setAgentId(data.target.agentId);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [parentId]);
  return agentId;
}

/**
 * Where a fork begins (`vymalo.fork`, docs/api/agui.md "Forks"): "Forked from <title>", a link to
 * the parent while it still exists, and "continued with <agent>" when the fork talks to another
 * agent than its parent did. An edit says nothing here: the branch picker under the message says
 * it (an edit is a version of a message, not a conversation of its own).
 */
export function ForkDivider({ fork }: { fork: ForkContent }) {
  const { forkedFrom } = useThreadView();
  // the resource drops the parent's id when the parent is deleted; the marker keeps it for good
  const parentId = forkedFrom?.threadId;
  const parentAgent = useParentAgent(fork.kind === "fork" ? parentId : undefined);
  if (fork.kind !== "fork") return null;
  const other = parentAgent !== null && parentAgent !== fork.target.agentId;
  const title = fork.title || "another chat";
  return (
    <MessagePrimitive.Root
      data-slot="fork-divider"
      className="flex min-w-0 motion-safe:animate-turn-in items-center gap-3 text-[0.8125rem] text-muted-foreground"
    >
      <span aria-hidden="true" className="h-px flex-1 bg-border" />
      <p className="flex min-w-0 max-w-full flex-wrap items-center justify-center gap-x-1.5 text-center">
        <GitForkIcon aria-hidden="true" className="size-3.5 shrink-0" />
        <span>
          Forked from{" "}
          {parentId ? (
            <Link
              href={`/threads/${parentId}`}
              className="font-medium text-foreground underline underline-offset-2 [overflow-wrap:anywhere]"
            >
              {title}
            </Link>
          ) : (
            <span className="font-medium text-foreground [overflow-wrap:anywhere]">{title}</span>
          )}
          {other ? (
            <>
              {" "}
              · continued with <span className="capitalize">{fork.target.agentId}</span>
            </>
          ) : null}
        </span>
      </p>
      <span aria-hidden="true" className="h-px flex-1 bg-border" />
    </MessagePrimitive.Root>
  );
}
