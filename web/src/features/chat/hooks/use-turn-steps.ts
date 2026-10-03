import { useAuiState } from "@assistant-ui/react";
import { useMemo } from "react";
import { useAgentNames } from "@/features/agents/components/agent-names-context";
import { useThreadView } from "@/features/chat/components/thread-view";
import { buildTurnSteps, type StepMessage, type TurnSteps } from "@/features/chat/lib/step-tree";

/**
 * The agent turns of the thread the page is in, each with its tree: read from the messages the
 * runtime already holds, so the chat's one line per turn and the panel's tree never disagree and
 * nothing is fetched. A turn is rebuilt only when its message changed (`buildTurnSteps`). It must
 * be used inside the thread page's `AssistantRuntimeProvider` and `ThreadViewProvider`.
 */
export function useTurnSteps(): readonly TurnSteps[] {
  const messages = useAuiState((s) => s.thread.messages) as readonly StepMessage[];
  const { state, waiting, agentId } = useThreadView();
  const agentNames = useAgentNames();
  return useMemo(
    () => buildTurnSteps(messages, { state, waiting, agentId, agentNames }),
    [messages, state, waiting, agentId, agentNames],
  );
}
