import { RotateCcwIcon } from "lucide-react";
import type { ReworkContent } from "@/features/chat/lib/agui/vymalo";
import { pluralFindings } from "@/features/chat/lib/findings";

/**
 * The gate failed and the agent is sent back (ADR 0018): a divider in the transcript between the
 * attempt that failed and the one that starts. The findings themselves are on the check cards above.
 */
export function ReworkDivider({ data }: { data: ReworkContent }) {
  const count = data.findings.reduce((sum, f) => sum + f.findings.length, 0);
  return (
    <div
      data-slot="rework-divider"
      data-attempt={data.attempt}
      className="flex w-full items-center gap-3 py-1 text-sm text-muted-foreground"
    >
      <span aria-hidden="true" className="h-px flex-1 bg-border" />
      <p className="flex min-w-0 flex-wrap items-center justify-center gap-x-1.5 text-center font-medium">
        <RotateCcwIcon aria-hidden="true" className="size-3.5 shrink-0" />
        <span>{`Attempt ${data.attempt} of ${data.maxAttempts}: sent back with ${pluralFindings(count)}`}</span>
      </p>
      <span aria-hidden="true" className="h-px flex-1 bg-border" />
    </div>
  );
}
