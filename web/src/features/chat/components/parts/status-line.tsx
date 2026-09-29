import type { StatusContent } from "@/features/chat/lib/agui/vymalo";
import type { AgentStatus } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { ActorLabel } from "../actor-label";

/** The label of every agent status the UI knows; golden.test.ts checks transcripts against it. */
export const STATUS_TEXT: Record<AgentStatus, string> = {
  submitted: "Submitted",
  working: "Working",
  input_required: "Needs input",
  auth_required: "Authentication required",
  completed: "Completed",
  failed: "Failed",
  canceled: "Cancelled",
};

/** The colour of the whole line (`text-*`) or of its dot only (`dot`). */
const TONE: Record<AgentStatus, { line?: string; dot?: string }> = {
  submitted: {},
  working: { dot: "text-primary" },
  input_required: { line: "text-warning" },
  auth_required: { line: "text-warning" },
  completed: { dot: "text-success" },
  failed: { line: "text-destructive" },
  canceled: {},
};

/** One compact, muted line per `agent_status` event. */
export function StatusLine({ data }: { data: StatusContent }) {
  const label = STATUS_TEXT[data.status];
  const tone = TONE[data.status];
  return (
    <p
      className={cn("flex flex-wrap items-baseline gap-2 text-sm text-muted-foreground", tone.line)}
    >
      <span
        className={cn(
          "size-1.5 shrink-0 -translate-y-px self-center rounded-full bg-current",
          tone.dot,
        )}
        aria-hidden="true"
      />
      <span className="min-w-0 [overflow-wrap:anywhere]">
        {label}
        {data.detail ? `: ${data.detail}` : ""}
      </span>
      <ActorLabel actor={data.actor} />
    </p>
  );
}
