import { useMemo } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import type { ApiThread } from "@/lib/api/types";
import { useSessions } from "../components/sessions-provider";
import { type ChildView, childView } from "../lib/child-view";

/** The open thread seen as a subagent's own, or `null` when it is nobody's helper (and always `null` for the new-chat page). */
export function useChildView(thread: ApiThread | null, threadId: string | null): ChildView | null {
  const { self } = useThreadView();
  const applied = useSessions()?.applied.get(threadId ?? "");
  return useMemo(
    () => (threadId === null ? null : childView({ thread, threadId, self: self ?? null, applied })),
    [thread, threadId, self, applied],
  );
}
