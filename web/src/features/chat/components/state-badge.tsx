import {
  BanIcon,
  CheckIcon,
  LoaderCircleIcon,
  type LucideIcon,
  MessageCircleQuestionIcon,
  TriangleAlertIcon,
  XIcon,
} from "lucide-react";
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
  queued: "bg-primary/10 text-primary",
  working: "bg-primary/10 text-primary",
  verifying: "bg-verifying/10 text-verifying",
  blocked: "bg-warning-soft text-warning",
  done: "bg-success/10 text-success",
  failed: "bg-destructive-soft text-destructive",
  cancelled: "bg-muted text-muted-foreground",
};

const ICON: Record<ThreadState, LucideIcon> = {
  queued: LoaderCircleIcon,
  working: LoaderCircleIcon,
  verifying: LoaderCircleIcon,
  blocked: TriangleAlertIcon,
  done: CheckIcon,
  failed: XIcon,
  cancelled: BanIcon,
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
  const yourTurn = state === "blocked" && needsAnswer;
  const label = yourTurn ? YOUR_TURN : LABELS[state];
  const Icon = yourTurn ? MessageCircleQuestionIcon : ICON[state];
  const spins = state === "queued" || state === "working" || state === "verifying";
  return (
    <span
      role="status"
      data-slot="state-badge"
      aria-label={`Thread state: ${SPOKEN[state] ?? label}`}
      className={cn(
        "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full px-2.5 text-[0.8125rem] font-medium whitespace-nowrap",
        TONE[state],
      )}
    >
      <Icon
        aria-hidden="true"
        className={cn("size-3.5", spins && "motion-safe:animate-spin")}
        strokeWidth={2.25}
      />
      {label}
    </span>
  );
}

export const stateLabel = (state: ThreadState): string => LABELS[state];
