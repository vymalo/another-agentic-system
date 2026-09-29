import type { ThreadState } from "@/lib/api/types";

const LABELS: Record<ThreadState, string> = {
  queued: "Queued",
  working: "Working",
  blocked: "Waiting for you",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

/** Text label first, colour second: state is never conveyed by colour alone. */
export function StateBadge({ state }: { state: ThreadState | undefined }) {
  if (!state) return null;
  return (
    <span
      className={`badge badge--${state}`}
      role="status"
      aria-label={`Thread state: ${LABELS[state]}`}
    >
      <span className="badge__dot" aria-hidden="true" />
      {LABELS[state]}
    </span>
  );
}

export const stateLabel = (state: ThreadState): string => LABELS[state];
