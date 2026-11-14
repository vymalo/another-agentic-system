"use client";

import { XIcon } from "lucide-react";
import { Hint } from "@/components/hint";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useEarlierControl } from "@/features/chat/components/earlier";
import { usePanel } from "../hooks/use-panel";
import { useThreadPlan } from "../hooks/use-plan";
import { useSources, useTurnsHeld } from "../hooks/use-sources";
import { parseTab } from "../lib/panel-state";
import { countSources } from "../lib/sources";
import { ActivityTab } from "./activity-tab";
import { PlanTab, planCount } from "./plan-tab";
import { SourcesView } from "./sources-tab";

/**
 * What the panel holds, docked or in a sheet: its tabs, Activity and Sources (with how many), and a
 * close button. The tab is the person's (remembered); the arrows, Home and End move between tabs.
 */
export function PanelBody({ onClose }: { onClose: () => void }) {
  const panel = usePanel();
  const groups = useSources(panel?.open ?? false);
  // the transcript of a thread opened at its end holds the newest turns: Sources say so, and can load the rest
  const earlier = useEarlierControl();
  const turnsHeld = useTurnsHeld(panel?.open ?? false);
  // the plan of the thread, when it has one: a third tab (docs/api/ui-catalog-v1.md, "The plan")
  const plan = useThreadPlan();
  if (!panel) return null;
  const count = countSources(groups);
  const planCountText = planCount(plan);
  // a plan that is gone leaves the tab a person had open on it on the first one
  const tab = panel.tab === "plan" && !plan ? "activity" : panel.tab;
  return (
    <Tabs
      value={tab}
      onValueChange={(value) => panel.setTab(parseTab(value) ?? "activity")}
      className="min-h-0 flex-1"
    >
      <div className="flex h-12 shrink-0 items-stretch justify-between gap-2 border-b ps-4 pe-2">
        <TabsList aria-label="Sections">
          <TabsTrigger value="activity">Activity</TabsTrigger>
          <TabsTrigger value="sources">
            Sources
            {count > 0 ? (
              <span className="text-xs font-normal text-muted-foreground tabular-nums">
                {count}
                {earlier?.earlier ? (
                  <>
                    +<span className="sr-only"> (earlier turns are not loaded)</span>
                  </>
                ) : null}
              </span>
            ) : null}
          </TabsTrigger>
          {plan ? (
            <TabsTrigger value="plan">
              Plan
              {planCountText ? (
                <span className="text-xs font-normal text-muted-foreground tabular-nums">
                  {planCountText}
                </span>
              ) : null}
            </TabsTrigger>
          ) : null}
        </TabsList>
        <Hint label="Close details">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label="Close details"
            onClick={onClose}
            className="size-8 self-center rounded-full text-muted-foreground hover:text-foreground"
          >
            <XIcon aria-hidden="true" className="size-4.5" />
          </Button>
        </Hint>
      </div>
      <TabsContent value="activity" className="overflow-y-auto overscroll-contain">
        <ActivityTab />
      </TabsContent>
      <TabsContent value="sources" className="overflow-y-auto overscroll-contain">
        <SourcesView
          groups={groups}
          onShowTurn={panel.showTurn}
          {...(earlier?.earlier
            ? {
                window: {
                  turns: turnsHeld,
                  state: earlier.state,
                  error: earlier.error,
                  onLoadAll: earlier.loadAll,
                },
              }
            : {})}
        />
      </TabsContent>
      {plan ? (
        <TabsContent value="plan" className="overflow-y-auto overscroll-contain">
          <PlanTab plan={plan} />
        </TabsContent>
      ) : null}
    </Tabs>
  );
}
