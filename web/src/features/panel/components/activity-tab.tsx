import { EmptyPanel } from "./empty-panel";

/**
 * The agents' work, turn by turn. The step tree (plan 03, S5.4) is mounted here: its connected
 * component reads `useStepsPanel()` (hooks/use-steps-panel.tsx), which is the whole contract
 * between it and this shell, and fills the tab with one section per agent turn. Until it exists the
 * steps stay in the conversation, under each agent's name, and this says so.
 */
export function ActivityTab() {
  return (
    <EmptyPanel title="What the agents do shows up here">
      For now, each step is listed in the conversation, under the agent&rsquo;s name.
    </EmptyPanel>
  );
}
