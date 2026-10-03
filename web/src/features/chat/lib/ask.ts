import type { AskContent, AskState } from "@/features/chat/lib/agui/vymalo";

/*
 * How an asked agent reads in the step tree (web/DESIGN.md, "Asked agents"): the words of each
 * state, so that the state is text and never colour alone, and the one note a line keeps even
 * collapsed. Pure; the line itself is `components/steps/step-node.tsx`.
 */

/** What each state of an ask says on its line. */
export const ASK_WORD: Record<AskState, string> = {
  running: "Working",
  completed: "Answered",
  input_required: "Asked back",
  auth_required: "Needs sign-in",
  failed: "Failed",
  rejected: "Refused",
  canceled: "Stopped",
  timed_out: "Timed out",
};

/**
 * The words of a line: its state's, except for an ask the log never ended (its turn is over, or
 * paused, so nothing runs it): that one is stopped, or waiting with its turn.
 */
export function askWord(state: AskState, step: "running" | "waiting" | string): string {
  if (state === "running" && step === "canceled") return ASK_WORD.canceled;
  if (state === "running" && step === "waiting") return "Waiting";
  return ASK_WORD[state];
}

/** The states in which the ask did not do what was asked of it: a line that says why, collapsed too. */
export const ASK_NOT_DONE: ReadonlySet<AskState> = new Set([
  "failed",
  "rejected",
  "canceled",
  "timed_out",
]);

/**
 * What a line says under it, open or not: the question an agent asked back (`input_required`,
 * `auth_required`), or why the ask did not complete. Untrusted text, drawn as text. Nothing for an
 * ask that is running or answered, and nothing when the agent gave no words.
 */
export function askNote(
  content: Pick<AskContent, "state" | "question" | "error">,
): { kind: "question" | "error"; text: string } | undefined {
  if (content.state === "input_required" || content.state === "auth_required") {
    return content.question?.trim() ? { kind: "question", text: content.question } : undefined;
  }
  if (ASK_NOT_DONE.has(content.state) && content.error?.trim()) {
    return { kind: "error", text: content.error };
  }
  return undefined;
}
