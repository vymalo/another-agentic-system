import { CheckIcon, LoaderCircleIcon, XIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Card, CardAction, CardContent, CardHeader } from "@/components/ui/card";
import type { CheckContent, CheckStatus } from "@/features/chat/lib/agui/vymalo";
import { shortCommit, sourceLabel } from "@/features/chat/lib/findings";
import { cn } from "@/lib/utils";
import { ActorLabel } from "../actor-label";
import { FindingsList } from "./findings-list";

const STATUS_TEXT: Record<CheckStatus, string> = {
  pending: "Pending",
  passed: "Passed",
  failed: "Failed",
};

/** Text and an icon carry the status; the colour only backs them up. */
const STATUS_TONE: Record<CheckStatus, string> = {
  pending: "text-verifying",
  passed: "text-success",
  failed: "text-destructive",
};

function StatusIcon({ status }: { status: CheckStatus }) {
  if (status === "passed") return <CheckIcon aria-hidden="true" />;
  if (status === "failed") return <XIcon aria-hidden="true" />;
  return <LoaderCircleIcon aria-hidden="true" className="motion-safe:animate-spin" />;
}

/**
 * One source of the verification gate for one attempt (ADR 0018): passed, failed or pending, the
 * source, the commit it was about and what it found. It is replaced in place when the source
 * answers again (`check-<attempt>-<verification>-<source>`). A stale answer, one for a verification that is no
 * longer the current one, is a card of its own and is shown muted: it decided nothing.
 *
 * `summary`, `name`, `commit` and every finding are untrusted text and are drawn as text.
 */
export function CheckCard({ data }: { data: CheckContent }) {
  const label = sourceLabel(data.source);
  const status = STATUS_TEXT[data.status];
  return (
    <Card
      size="sm"
      role="region"
      aria-label={`Check: ${label}, attempt ${data.attempt}, ${status.toLowerCase()}${data.stale ? ", stale" : ""}`}
      data-slot="check-card"
      data-source={data.source}
      data-status={data.status}
      data-stale={data.stale ? "true" : undefined}
      className={cn(
        "w-full max-w-md shadow-none",
        data.stale && "border border-dashed bg-muted/40 text-muted-foreground ring-0",
      )}
    >
      <CardHeader>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <Badge
            variant="outline"
            className={cn(
              "border-current font-semibold",
              data.stale ? "text-muted-foreground" : STATUS_TONE[data.status],
            )}
          >
            <StatusIcon status={data.status} />
            {status}
          </Badge>
          <span
            className={cn(
              "font-semibold [overflow-wrap:anywhere]",
              data.stale ? "text-muted-foreground" : "text-foreground",
            )}
          >
            {label}
            {data.name ? <span className="font-normal"> · {data.name}</span> : null}
          </span>
          {data.stale ? (
            <Badge variant="secondary" className="border border-current text-muted-foreground">
              Stale
            </Badge>
          ) : null}
        </div>
        <CardAction>
          <ActorLabel actor={data.actor} />
        </CardAction>
      </CardHeader>
      <CardContent className="gap-2">
        <p className="flex flex-wrap items-baseline gap-x-2 text-xs text-muted-foreground">
          <span>Attempt {data.attempt}</span>
          {data.commit ? (
            <code title={data.commit} className="rounded bg-muted px-1 py-0.5 font-mono">
              {shortCommit(data.commit)}
            </code>
          ) : null}
        </p>
        {data.stale ? (
          <p className="text-xs">
            This answer came after the verification moved on. It decided nothing.
          </p>
        ) : null}
        {data.summary ? <p className="[overflow-wrap:anywhere]">{data.summary}</p> : null}
        <FindingsList findings={data.findings} />
      </CardContent>
    </Card>
  );
}
