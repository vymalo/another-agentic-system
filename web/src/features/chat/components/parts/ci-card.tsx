import {
  BanIcon,
  CheckIcon,
  CircleQuestionMarkIcon,
  ClockAlertIcon,
  ExternalLinkIcon,
  GitBranchIcon,
  HistoryIcon,
  MinusIcon,
  OctagonAlertIcon,
  SkipForwardIcon,
  TriangleAlertIcon,
  XIcon,
} from "lucide-react";
import type { ComponentType } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardHeader } from "@/components/ui/card";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import type { CiContent } from "@/features/chat/lib/agui/vymalo";
import {
  type CiConclusion,
  conclusionLabel,
  isKnownConclusion,
  providerLabel,
} from "@/features/chat/lib/ci";
import { shortCommit, truncate } from "@/features/chat/lib/findings";
import { cn } from "@/lib/utils";
import { ActorLabel } from "../actor-label";
import { ExpandableText } from "./expandable-text";

/** A summary longer than this is cut, with a control to read the rest (up to 16 KiB arrive). */
export const SUMMARY_PREVIEW = 240;
/** A check name or a branch longer than this is cut; the card is not the place for a novel. */
export const NAME_PREVIEW = 120;

type Icon = ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;

/** The words carry the conclusion; the icon and the colour only back them up. */
const LOOK: Record<CiConclusion, { icon: Icon; tone: string }> = {
  success: { icon: CheckIcon, tone: "text-success" },
  neutral: { icon: MinusIcon, tone: "text-muted-foreground" },
  skipped: { icon: SkipForwardIcon, tone: "text-muted-foreground" },
  failure: { icon: XIcon, tone: "text-destructive" },
  cancelled: { icon: BanIcon, tone: "text-muted-foreground" },
  timed_out: { icon: ClockAlertIcon, tone: "text-destructive" },
  action_required: { icon: TriangleAlertIcon, tone: "text-warning" },
  stale: { icon: HistoryIcon, tone: "text-muted-foreground" },
  startup_failure: { icon: OctagonAlertIcon, tone: "text-destructive" },
};

/** A conclusion this UI does not know: `passed`, which the orchestrator computed, picks the colour. */
function lookOf(data: CiContent): { icon: Icon; tone: string } {
  if (isKnownConclusion(data.conclusion)) return LOOK[data.conclusion];
  return {
    icon: CircleQuestionMarkIcon,
    tone: data.passed ? "text-success" : "text-destructive",
  };
}

/**
 * A CI system's report on a commit (ADR 0017): the conclusion in words, the check, the commit, where
 * it ran and, when the report has one, a link to the run. It is replaced in place when the same
 * check runs again on the same commit (`ci-<sha>-<name>`). It is the orchestrator's, not the agent's.
 *
 * `name`, `branch`, `summary` and the rest come from whoever runs the CI: untrusted text, drawn as
 * text. The link is drawn only when it is an absolute http(s) URL, checked here again.
 */
export function CiCard({ data }: { data: CiContent }) {
  const { icon: ConclusionIcon, tone } = lookOf(data);
  const label = conclusionLabel(data.conclusion);
  const name = truncate(data.name, NAME_PREVIEW);
  const branch = data.branch ? truncate(data.branch, NAME_PREVIEW) : undefined;
  const href = safeHttpUrl(data.url);
  return (
    <Card
      size="sm"
      role="region"
      aria-label={`CI: ${name.text}, ${label.toLowerCase()}`}
      data-slot="ci-card"
      data-conclusion={data.conclusion}
      data-passed={data.passed ? "true" : "false"}
      className="w-full max-w-md shadow-none"
    >
      <CardHeader>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <Badge variant="outline" className={cn("border-current font-semibold", tone)}>
            <ConclusionIcon aria-hidden="true" />
            {label}
          </Badge>
          <span
            data-slot="ci-name"
            title={name.cut ? data.name : undefined}
            className="font-semibold text-foreground [overflow-wrap:anywhere]"
          >
            {name.text}
          </span>
        </div>
        <CardAction>
          <ActorLabel actor={data.actor} />
        </CardAction>
      </CardHeader>
      <CardContent className="gap-2">
        <p className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
          <code title={data.sha} className="rounded bg-muted px-1 py-0.5 font-mono">
            {shortCommit(data.shortSha)}
          </code>
          {branch ? (
            <span data-slot="ci-branch" title={branch.cut ? data.branch : undefined}>
              <GitBranchIcon aria-hidden="true" className="mr-0.5 inline size-3 align-[-0.125em]" />
              <span className="sr-only">Branch </span>
              <span className="[overflow-wrap:anywhere]">{branch.text}</span>
            </span>
          ) : null}
        </p>
        <p data-slot="ci-source" className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
          {providerLabel(data.provider)} · {truncate(data.repository, NAME_PREVIEW).text}
        </p>
        {data.summary ? (
          <p data-slot="ci-summary" className="[overflow-wrap:anywhere]">
            <ExpandableText text={data.summary} limit={SUMMARY_PREVIEW} />
          </p>
        ) : null}
        {href ? (
          <Button
            asChild
            variant="link"
            size="xs"
            className="h-auto justify-start self-start px-0 font-semibold"
          >
            <a href={href} target="_blank" rel="noopener noreferrer">
              View run
              <ExternalLinkIcon aria-hidden="true" />
              <span className="sr-only"> (opens in a new tab)</span>
            </a>
          </Button>
        ) : null}
      </CardContent>
    </Card>
  );
}
