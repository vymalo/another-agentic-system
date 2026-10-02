import { useAuiState } from "@assistant-ui/react";
import { useMemo, useRef } from "react";
import { type FileMessage, filesOf, type KeptFile, sameFiles } from "@/features/chat/lib/files";

const NONE: readonly KeptFile[] = [];

/**
 * The files the thread holds, from the messages the runtime has. The runtime hands over the whole
 * transcript on every streamed word, so the list keeps its identity while the files are the same:
 * what depends on it (a surface's validation) does not run again for a word.
 */
export function useThreadFilesList(): readonly KeptFile[] {
  const messages = useAuiState((s) => s.thread.messages as readonly FileMessage[]);
  const next = useMemo(() => filesOf(messages), [messages]);
  const last = useRef<readonly KeptFile[]>(NONE);
  if (!sameFiles(last.current, next)) last.current = next;
  return last.current;
}
