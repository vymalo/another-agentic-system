"use client";

import type { ReactNode } from "react";
import type { ApiThread } from "@/lib/api/types";
import { useChildView } from "../hooks/use-child-view";
import type { ChildView } from "../lib/child-view";
import { SessionClosedNotice } from "./session-closed-notice";

/** What the message box needs of a subagent's thread. */
export type ChildBox = {
  keepOpen: boolean;
  /** The helper's name: who reads a message ("planner reads it at its next step"). */
  agent: string;
  /** The session is closing: its round ends first, or the close is on its way. */
  closing: boolean;
};

export const childBox = (view: ChildView): ChildBox => ({
  keepOpen: view.keepOpen,
  agent: view.agent || "The subagent",
  closing: view.phase === "closing",
});

/**
 * The place of the message box in a subagent's thread: the box (what `children` draws, told what a
 * subagent changes in it) while the session takes messages, and, once it is closed, the notice that says
 * so, with the way back. Any other thread gets its box as it always had it.
 */
export function ChildComposerGate({
  thread,
  threadId,
  rows,
  children,
}: {
  thread: ApiThread | null;
  threadId: string;
  rows: readonly ApiThread[];
  children: (child: ChildBox | null) => ReactNode;
}) {
  const view = useChildView(thread, threadId);
  if (view === null) return <>{children(null)}</>;
  if (view.phase === "closed") return <SessionClosedNotice parent={view.parent} rows={rows} />;
  return <>{children(childBox(view))}</>;
}
