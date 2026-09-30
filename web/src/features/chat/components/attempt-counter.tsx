import type { JobView } from "@/features/chat/lib/agui/vymalo";

/**
 * "Attempt 2/3": which try the agent is on under a verification gate (ADR 0018). Drawn only when
 * the server sent a `job`, so a thread without a gate looks as it always did. Screen readers get
 * the words, not the slash.
 */
export function AttemptCounter({ job }: { job: JobView | null }) {
  if (!job) return null;
  return (
    <span data-slot="attempt-counter" className="text-sm whitespace-nowrap text-muted-foreground">
      <span aria-hidden="true" className="tabular-nums">
        Attempt {job.attempt}/{job.maxAttempts}
      </span>
      <span className="sr-only">
        Attempt {job.attempt} of {job.maxAttempts}
      </span>
    </span>
  );
}
