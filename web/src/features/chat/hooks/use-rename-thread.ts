import type { ApiThread } from "@/lib/api/types";
import { type ThreadTextEditor, useThreadEdit } from "./use-thread-edit";

export type ThreadRenamer = ThreadTextEditor;

/**
 * Renaming the open thread in its header (`PATCH /api/threads/{id}` with `title`): a person's
 * title is final. The same title or an empty one is no rename (see `useThreadEdit`).
 */
export function useRenameThread(
  thread: ApiThread | null,
  onRenamed: (thread: ApiThread) => void,
): ThreadRenamer {
  return useThreadEdit(thread, onRenamed, { member: "title", allowEmpty: false });
}
