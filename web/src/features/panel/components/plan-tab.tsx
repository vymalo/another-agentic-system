"use client";

import { ActorLabel } from "@/features/chat/components/actor-label";
import { SurfaceNewer, SurfaceRefused } from "@/features/chat/components/surface/surface-activity";
import { SurfaceView } from "@/features/chat/components/surface/surface-view";
import { ThreadFilesContext } from "@/features/chat/components/surface/thread-files";
import { useThreadFilesList } from "@/features/chat/hooks/use-thread-files";
import type { ThreadPlan } from "../hooks/use-plan";

/** What the Plan tab's name says of the plan: `2/5`, from the plan's checklists together. */
export const planCount = (plan: ThreadPlan | undefined): string | undefined => {
  const p = plan?.prepared;
  return p?.kind === "surface" && p.progress && p.progress.total > 0
    ? `${p.progress.done}/${p.progress.total}`
    : undefined;
};

/**
 * The plan of the thread, pinned in the panel (docs/api/ui-catalog-v1.md, "The plan", and web/DESIGN.md,
 * "Panel"): the newest copy of the latest plan, the very one the conversation draws where the plan first
 * stood. It only reads: the conversation's copy is the one that says, politely, that the plan changed.
 */
export function PlanTab({ plan }: { plan: ThreadPlan }) {
  const files = useThreadFilesList();
  const { prepared } = plan;
  return (
    <div data-slot="plan-tab" className="flex min-w-0 flex-col gap-3 p-4">
      {prepared.kind === "refused" ? (
        <SurfaceRefused
          rule={prepared.rule}
          reason={prepared.reason}
          operations={plan.operations}
          actor={plan.actor}
        />
      ) : prepared.kind === "newer" ? (
        <SurfaceNewer component={prepared.component} actor={plan.actor} />
      ) : prepared.kind === "surface" ? (
        <>
          <ThreadFilesContext.Provider value={files}>
            <SurfaceView
              prepared={prepared}
              live={false}
              fallback={
                <SurfaceRefused
                  rule="render"
                  reason="the plan failed to draw"
                  operations={plan.operations}
                  actor={plan.actor}
                />
              }
            />
          </ThreadFilesContext.Provider>
          <ActorLabel actor={plan.actor} />
        </>
      ) : (
        <p className="text-xs text-muted-foreground">The plan is not complete yet.</p>
      )}
    </div>
  );
}
