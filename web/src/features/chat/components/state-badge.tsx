import { Badge } from "@/components/ui/badge";
import type { ThreadState } from "@/lib/api/types";
import { cn } from "@/lib/utils";

const LABELS: Record<ThreadState, string> = {
  queued: "Queued",
  working: "Working",
  verifying: "Verifying",
  blocked: "Waiting for you",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

const TONE: Record<ThreadState, string> = {
  queued: "text-primary",
  working: "text-primary",
  verifying: "text-primary",
  blocked: "text-warning",
  done: "text-success",
  failed: "text-destructive",
  cancelled: "text-muted-foreground",
};

/** Text label first, colour second: state is never conveyed by colour alone. */
export function StateBadge({ state }: { state: ThreadState | undefined }) {
  if (!state) return null;
  return (
    <Badge
      variant="outline"
      role="status"
      aria-label={`Thread state: ${LABELS[state]}`}
      className={cn(
        "h-6 gap-1.5 border-current px-2.5 text-[0.8125rem] font-semibold",
        TONE[state],
      )}
    >
      <span className="size-2 rounded-full bg-current" aria-hidden="true" />
      {LABELS[state]}
    </Badge>
  );
}

export const stateLabel = (state: ThreadState): string => LABELS[state];
