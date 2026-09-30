import { Badge } from "@/components/ui/badge";
import type { ThreadState } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/**
 * One pill, in words a person uses (a thread is a conversation, not a job queue). A blocked thread
 * is "Your turn" when the agent asked something (an interrupt is open), "Needs attention" when it
 * waits for another reason (a hold, a delivery error).
 */
const LABELS: Record<ThreadState, string> = {
  queued: "Starting…",
  working: "Working…",
  verifying: "Checking the work…",
  blocked: "Needs attention",
  done: "Done",
  failed: "Failed",
  cancelled: "Stopped",
};

const YOUR_TURN = "Your turn";

const TONE: Record<ThreadState, string> = {
  queued: "text-primary",
  working: "text-primary",
  verifying: "text-verifying",
  blocked: "text-warning",
  done: "text-success",
  failed: "text-destructive",
  cancelled: "text-muted-foreground",
};

/**
 * What a screen reader hears after "Thread state:" when the label alone says too little. Under a
 * verification gate (ADR 0018) the agent is done and the orchestrator is checking its work.
 */
const SPOKEN: Partial<Record<ThreadState, string>> = {
  verifying: "Checking the agent's work",
};

/** Text label first, colour second: state is never conveyed by colour alone. */
export function StateBadge({
  state,
  needsAnswer = false,
}: {
  state: ThreadState | undefined;
  /** The agent asked a question and waits for the answer (an interrupt is open). */
  needsAnswer?: boolean;
}) {
  if (!state) return null;
  const label = state === "blocked" && needsAnswer ? YOUR_TURN : LABELS[state];
  return (
    <Badge
      variant="outline"
      role="status"
      aria-label={`Thread state: ${SPOKEN[state] ?? label}`}
      className={cn(
        "h-6 gap-1.5 border-current px-2.5 text-[0.8125rem] font-semibold",
        TONE[state],
      )}
    >
      <span className="size-2 rounded-full bg-current" aria-hidden="true" />
      {label}
    </Badge>
  );
}

export const stateLabel = (state: ThreadState): string => LABELS[state];
