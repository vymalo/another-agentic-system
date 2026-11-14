import { useAuiState } from "@assistant-ui/react";
import { useMemo } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import { useThreadFilesList } from "@/features/chat/hooks/use-thread-files";
import { type Prepared, prepareSurface } from "@/features/chat/lib/a2ui/prepare";
import { latestPlan } from "@/features/chat/lib/a2ui/surfaces";
import { parseSurface, type SurfaceContent } from "@/features/chat/lib/agui/vymalo";

/** The plan of the thread as the panel shows it: its newest copy, validated. */
export type ThreadPlan = {
  prepared: Prepared;
  operations: unknown;
  actor: SurfaceContent["actor"];
};

/**
 * The latest plan of the thread (docs/api/ui-catalog-v1.md, "The plan"): the newest copy of the plan
 * surface that was sent last, run through the same validator as the copy in the conversation. `undefined`
 * while the thread has none, and when the agent deleted it. Nothing is read while nobody can see the tab
 * (`enabled` false) except whether a plan exists, which is what the tab's name needs.
 */
export function useThreadPlan(): ThreadPlan | undefined {
  const data = useAuiState((s) => latestPlan(s.thread.messages)?.data);
  const { catalogVersion } = useThreadView();
  const files = useThreadFilesList();
  return useMemo(() => {
    const content = parseSurface(data);
    if (!content) return undefined;
    const prepared = prepareSurface(content.a2ui_operations, {
      threadVersion: catalogVersion,
      files,
    });
    if (prepared.kind === "deleted") return undefined;
    return { prepared, operations: content.a2ui_operations, actor: content.actor };
  }, [data, catalogVersion, files]);
}
