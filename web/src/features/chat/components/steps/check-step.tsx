import { CheckIcon, HistoryIcon, LoaderCircleIcon, ShieldCheckIcon, XIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { CheckContent, CheckStatus } from "@/features/chat/lib/agui/vymalo";
import { shortCommit, sourceLabel } from "@/features/chat/lib/findings";
import { checkLabel } from "@/features/chat/lib/steps";
import { cn } from "@/lib/utils";
import { ActorLabel } from "../actor-label";
import { FindingsDisclosure } from "./step-items";
import { StepRow, type StepState } from "./step-row";

const STATUS_TEXT: Record<CheckStatus, string> = {
  pending: "Pending",
  passed: "Passed",
  failed: "Failed",
};

/** Text and an icon carry the status; the colour only backs them up. */
const STATUS_TONE: Record<CheckStatus, string> = {
  pending: "bg-verifying/10 text-verifying",
  passed: "bg-success/10 text-success",
  failed: "bg-destructive-soft text-destructive",
};

const ROW_STATE: Record<CheckStatus, StepState> = {
  pending: "pending",
  passed: "done",
  failed: "failed",
};

function StatusIcon({ status }: { status: CheckStatus }) {
  if (status === "passed") return <CheckIcon aria-hidden="true" />;
  if (status === "failed") return <XIcon aria-hidden="true" />;
  return <LoaderCircleIcon aria-hidden="true" className="motion-safe:animate-spin" />;
}

/**
 * One source of the verification gate for one attempt (ADR 0018), as a step: what the source is
 * doing or said, a pill with its status, the commit, the summary and, folded, the findings. It is
 * replaced in place when the source answers again (`check-<attempt>-<verification>-<source>`). A
 * stale answer, one for a verification that is no longer the current one, is a muted step of its
 * own: it decided nothing. `waiting: false` says the run ended with the check unanswered.
 *
 * `summary`, `name`, `commit` and every finding are untrusted text and are drawn as text.
 */
export function CheckStep({ data, waiting = true }: { data: CheckContent; waiting?: boolean }) {
  const label = sourceLabel(data.source);
  const status = STATUS_TEXT[data.status];
  const unanswered = data.status === "pending" && !waiting;
  const state: StepState = data.stale ? "muted" : unanswered ? "muted" : ROW_STATE[data.status];
  return (
    <StepRow
      state={state}
      icon={data.stale ? HistoryIcon : data.status === "passed" ? ShieldCheckIcon : undefined}
      aria-label={`Check: ${label}, attempt ${data.attempt}, ${status.toLowerCase()}${data.stale ? ", stale" : ""}`}
      data-slot="check-card"
      data-source={data.source}
      data-status={data.status}
      data-stale={data.stale ? "true" : undefined}
      label={
        <>
          <span className={cn(data.stale && "text-muted-foreground")}>{checkLabel(data)}</span>
          <Badge
            variant="secondary"
            className={cn(
              "h-5 gap-1 font-medium",
              data.stale ? "bg-muted text-muted-foreground" : STATUS_TONE[data.status],
            )}
          >
            <StatusIcon status={data.status} />
            {status}
          </Badge>
          {data.stale ? (
            <Badge variant="outline" className="h-5 text-muted-foreground">
              Stale
            </Badge>
          ) : null}
          {data.actor?.type === "agent" ? <ActorLabel actor={data.actor} /> : null}
        </>
      }
    >
      <p className="flex flex-wrap items-baseline gap-x-2 text-xs text-muted-foreground">
        {data.name ? <span className="[overflow-wrap:anywhere]">{data.name}</span> : null}
        <span>Attempt {data.attempt}</span>
        {data.commit ? (
          <code title={data.commit} className="rounded bg-muted px-1 py-px font-mono">
            {shortCommit(data.commit)}
          </code>
        ) : null}
      </p>
      {data.stale ? (
        <p className="text-xs text-muted-foreground">
          This answer came after the verification moved on. It decided nothing.
        </p>
      ) : null}
      {unanswered ? (
        <p className="text-xs text-muted-foreground">No answer came before the run ended.</p>
      ) : null}
      {data.summary ? (
        <p className="text-[0.8125rem] text-foreground/85 [overflow-wrap:anywhere]">
          {data.summary}
        </p>
      ) : null}
      {data.findings.length > 0 ? <FindingsDisclosure findings={data.findings} /> : null}
    </StepRow>
  );
}
