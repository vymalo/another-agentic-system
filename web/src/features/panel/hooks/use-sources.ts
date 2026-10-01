import { useAuiState } from "@assistant-ui/react";
import { useMemo } from "react";
import { collectSources, type SourceGroup, type SourceMessage } from "../lib/sources";

const NONE: readonly SourceMessage[] = [];

/**
 * What the agents of this thread shared, from the messages the runtime holds. `enabled` is false
 * while nobody can see the result (the panel is closed): then nothing is read and nothing is parsed.
 */
export function useSources(enabled: boolean): SourceGroup[] {
  const messages = useAuiState((s) =>
    enabled ? (s.thread.messages as readonly SourceMessage[]) : NONE,
  );
  return useMemo(() => collectSources(messages), [messages]);
}
