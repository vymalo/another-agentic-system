import { useAuiState } from "@assistant-ui/react";
import { useMemo } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import { agentTurns, collectSources, type SourceGroup, type SourceMessage } from "../lib/sources";

const NONE: readonly SourceMessage[] = [];

/**
 * What the agents of this thread shared, from the messages the runtime holds. `enabled` is false
 * while nobody can see the result (the panel is closed): then nothing is read and nothing is parsed.
 */
export function useSources(enabled: boolean): SourceGroup[] {
  const messages = useAuiState((s) =>
    enabled ? (s.thread.messages as readonly SourceMessage[]) : NONE,
  );
  const { turnsBefore } = useThreadView();
  return useMemo(() => collectSources(messages, turnsBefore ?? 0), [messages, turnsBefore]);
}

/**
 * How many agent turns the transcript holds: what the Sources of a thread opened at its end are drawn from, which says "from the
 * last N turns" while older ones are not loaded. Nothing is read while the panel is closed.
 */
export function useTurnsHeld(enabled: boolean): number {
  return useAuiState((s) =>
    enabled ? agentTurns(s.thread.messages as readonly SourceMessage[]).length : 0,
  );
}
