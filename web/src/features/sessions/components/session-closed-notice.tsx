"use client";

import type { ApiThread } from "@/lib/api/types";
import { ParentLink } from "./parent-link";

/** What replaces the message box of a subagent that is closed: it takes no more messages (a send is 409 `session_closed`). */
export function SessionClosedNotice({
  parent,
  rows,
}: {
  parent: string | undefined;
  rows: readonly ApiThread[];
}) {
  return (
    <div
      role="status"
      data-slot="session-closed-notice"
      className="flex flex-col items-start gap-1.5 rounded-3xl border bg-muted/40 px-4 py-3 text-sm"
    >
      <p>This subagent is closed and takes no more messages.</p>
      <ParentLink parent={parent} rows={rows} className="text-[0.8125rem]" />
    </div>
  );
}
