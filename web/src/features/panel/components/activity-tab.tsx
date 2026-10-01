import { StepsPanelContent } from "@/features/chat/components/steps/steps-panel-content";

/**
 * The agents' work, turn by turn: the step tree (web/DESIGN.md, "Steps panel"). Its connected
 * component reads the runtime's messages and `useStepsPanel()` (hooks/use-steps-panel.tsx), which
 * is the whole contract between it and this shell, and fills the tab with one section per agent
 * turn that did something; what is open is held above, by the shell, so closing the panel keeps it.
 */
export function ActivityTab() {
  return <StepsPanelContent />;
}
