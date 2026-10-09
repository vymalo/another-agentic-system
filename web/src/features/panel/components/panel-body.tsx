"use client";

import { XIcon } from "lucide-react";
import { Hint } from "@/components/hint";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { usePanel } from "../hooks/use-panel";
import { useSources } from "../hooks/use-sources";
import { parseTab } from "../lib/panel-state";
import { countSources } from "../lib/sources";
import { ActivityTab } from "./activity-tab";
import { SourcesView } from "./sources-tab";

/**
 * What the panel holds, docked or in a sheet: its tabs, Activity and Sources (with how many), and a
 * close button. The tab is the person's (remembered); the arrows, Home and End move between tabs.
 */
export function PanelBody({ onClose }: { onClose: () => void }) {
  const panel = usePanel();
  const groups = useSources(panel?.open ?? false);
  if (!panel) return null;
  const count = countSources(groups);
  return (
    <Tabs
      value={panel.tab}
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
              </span>
            ) : null}
          </TabsTrigger>
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
        <SourcesView groups={groups} onShowTurn={panel.showTurn} />
      </TabsContent>
    </Tabs>
  );
}
