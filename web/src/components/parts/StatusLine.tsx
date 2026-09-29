import type { AgentStatus } from "@/api/types";
import type { StatusPartData } from "@/chat/to-items";
import { ActorLabel } from "../ActorLabel";

/** The label of every agent status the UI knows; golden.test.ts checks transcripts against it. */
export const STATUS_TEXT: Record<AgentStatus, string> = {
  submitted: "Submitted",
  working: "Working",
  input_required: "Needs input",
  completed: "Completed",
  failed: "Failed",
  canceled: "Cancelled",
};

/** One compact, muted line per `agent_status` event. */
export function StatusLine({ data }: { data: StatusPartData }) {
  const label = STATUS_TEXT[data.status];
  return (
    <p className={`status-line status-line--${data.status}`}>
      <span className="status-line__dot" aria-hidden="true" />
      <span className="status-line__text">
        {label}
        {data.detail ? `: ${data.detail}` : ""}
      </span>
      <ActorLabel actor={data.actor} />
    </p>
  );
}
