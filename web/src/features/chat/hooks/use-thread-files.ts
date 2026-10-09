import { useAuiState } from "@assistant-ui/react";
import { useMemo, useRef } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import { type FileMessage, filesOf, type KeptFile, sameFiles } from "@/features/chat/lib/files";

const NONE: readonly KeptFile[] = [];

/**
 * The files the thread holds, from the messages the runtime has. The runtime hands over the whole
 * transcript on every streamed word, so the list keeps its identity while the files are the same:
 * what depends on it (a surface's validation) does not run again for a word.
 */
export function useThreadFilesList(): readonly KeptFile[] {
  const messages = useAuiState((s) => s.thread.messages as readonly FileMessage[]);
  const { carriedFiles } = useThreadView();
  const next = useMemo(() => {
    const held = filesOf(messages);
    if (!carriedFiles?.length) return held;
    // the files of the turns that are not held come first, as they were handed over; a hash is listed once
    const have = new Set(held.map((f) => f.sha256));
    return [...carriedFiles.filter((f) => !have.has(f.sha256)), ...held];
  }, [messages, carriedFiles]);
  const last = useRef<readonly KeptFile[]>(NONE);
  if (!sameFiles(last.current, next)) last.current = next;
  return last.current;
}
