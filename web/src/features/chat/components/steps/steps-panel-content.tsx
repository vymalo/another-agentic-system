"use client";

import { useThreadView } from "@/features/chat/components/thread-view";
import { useTurnSteps } from "@/features/chat/hooks/use-turn-steps";
import { useStepsExpansion, useStepsPanel } from "@/features/panel/hooks/use-steps-panel";
import { isActive } from "@/lib/api/types";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { StepsPane } from "./steps-pane";

/**
 * The connected form the panel's Activity tab renders: every agent turn of the thread (read from
 * the runtime's messages), what the shell asked to show, and the expansion state the shell keeps
 * above the panel. No props. It must sit inside the thread page's providers.
 */
export function StepsPanelContent() {
  const turns = useTurnSteps();
  const { focus } = useStepsPanel();
  const { state } = useThreadView();
  const [expanded, setExpanded] = useStepsExpansion<ExpansionState>(NO_EXPANSION);
  return (
    <StepsPane
      turns={turns}
      focus={focus}
      live={isActive(state)}
      expanded={expanded}
      onExpandedChange={setExpanded}
    />
  );
}
