import type { ApiThread } from "@/lib/api/types";
import { type ThreadTextEditor, useThreadEdit } from "./use-thread-edit";

export type ThreadDescriber = ThreadTextEditor;

/**
 * Writing the open thread's description in its header (`PATCH /api/threads/{id}` with
 * `description`, ADR 0035): a person's description is final, and an empty one clears it (the
 * model is not asked for it again either). The same text is no edit.
 */
export function useDescribeThread(
  thread: ApiThread | null,
  onDescribed: (thread: ApiThread) => void,
): ThreadDescriber {
  return useThreadEdit(thread, onDescribed, { member: "description", allowEmpty: true });
}
