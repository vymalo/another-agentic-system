import {
  ClockIcon,
  type LucideIcon,
  MessageCircleQuestionIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { Hint } from "@/components/hint";
import type { ThreadState } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { STATE_SHAPE } from "./state-shapes";

/**
 * One pill, in words a person uses (a thread is a conversation, not a job queue). A blocked thread
 * is "Your turn" when the agent asked something (an interrupt is open), "Needs attention" when it
 * waits for another reason (a hold, a delivery error). The words are the state's name and its tooltip.
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
  // neutral ink: the live dot and the live step carry the green, a badge only says it in words
  queued: "bg-muted text-foreground",
  working: "bg-muted text-foreground",
  verifying: "bg-verifying/10 text-verifying",
  blocked: "bg-warning-soft text-warning",
  done: "bg-success/10 text-success",
  failed: "bg-destructive-soft text-destructive",
  cancelled: "bg-muted text-muted-foreground",
};

const ICON: Record<ThreadState, LucideIcon> = {
  // waiting to start, then at work, then being checked: three shapes, so no state is told apart by
  // its colour or its motion alone (a person who asks for less motion gets them still)
  queued: ClockIcon,
  working: STATE_SHAPE.working,
  verifying: STATE_SHAPE.verifying,
  blocked: TriangleAlertIcon,
  done: STATE_SHAPE.done,
  failed: STATE_SHAPE.failed,
  cancelled: STATE_SHAPE.stopped,
};

/**
 * What a screen reader hears after "Thread state:" when the label alone says too little. Under a
 * verification gate (ADR 0018) the agent is done and the orchestrator is checking its work.
 */
const SPOKEN: Partial<Record<ThreadState, string>> = {
  verifying: "Checking the agent's work",
};

/**
 * The state is its icon, a shape of its own for each (a clock, a spinner, a shield with dots, a check, a
 * cross, a stop, a warning), and its words for a screen reader and the tooltip: a person who watches the thread knows a
 * check from a cross, and the colour only backs it. A thread that waits for the person keeps its words
 * on the pill, because it is the one state that asks them to do something.
 */
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
  const spins = state === "working";
  const pulses = state === "queued" || state === "verifying";
  const spoken = SPOKEN[state] ?? label;
  const words = state === "blocked";
  return (
    <Hint label={spoken}>
      <span
        role="status"
        data-slot="state-badge"
        aria-label={`Thread state: ${spoken}`}
        className={cn(
          "inline-flex h-7 shrink-0 items-center justify-center gap-1.5 rounded-full text-[0.8125rem] font-medium whitespace-nowrap",
          words ? "px-2.5" : "w-7",
          TONE[state],
        )}
      >
        <Icon
          aria-hidden="true"
          className={cn(
            "size-3.5",
            spins && "motion-safe:animate-spin",
            pulses && "motion-safe:animate-pulse",
          )}
          strokeWidth={2.25}
        />
        <span className={words ? undefined : "sr-only"}>{label}</span>
      </span>
    </Hint>
  );
}

export const stateLabel = (state: ThreadState): string => LABELS[state];
