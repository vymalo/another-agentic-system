import {
  BanIcon,
  CheckIcon,
  CircleDotIcon,
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
import { ExpandableText } from "../parts/expandable-text";
import { StepRow } from "./step-row";

/** A summary longer than this is cut, with a control to read the rest (up to 16 KiB arrive). */
export const SUMMARY_PREVIEW = 240;
/** A check name or a branch longer than this is cut; the card is not the place for a novel. */
export const NAME_PREVIEW = 120;

type Icon = ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;

/** The words carry the conclusion; the icon and the colour only back them up. */
const GOOD = "bg-success/10 text-success";
const BAD = "bg-destructive-soft text-destructive";
const QUIET = "bg-muted text-muted-foreground";

const LOOK: Record<CiConclusion, { icon: Icon; tone: string }> = {
  success: { icon: CheckIcon, tone: GOOD },
  neutral: { icon: MinusIcon, tone: QUIET },
  skipped: { icon: SkipForwardIcon, tone: QUIET },
  failure: { icon: XIcon, tone: BAD },
  cancelled: { icon: BanIcon, tone: QUIET },
  timed_out: { icon: ClockAlertIcon, tone: BAD },
  action_required: { icon: TriangleAlertIcon, tone: "bg-warning-soft text-warning" },
  stale: { icon: HistoryIcon, tone: QUIET },
  startup_failure: { icon: OctagonAlertIcon, tone: BAD },
};

/** A conclusion this UI does not know: `passed`, which the orchestrator computed, picks the colour. */
function lookOf(data: CiContent): { icon: Icon; tone: string } {
  if (isKnownConclusion(data.conclusion)) return LOOK[data.conclusion];
  return {
    icon: CircleQuestionMarkIcon,
    tone: data.passed ? GOOD : BAD,
  };
}

/**
 * A CI system's report on a commit (ADR 0017), as a step: the check, a pill with the conclusion in
 * words, the commit, the branch, where it ran and, when the report has one, a link to the run. It is
 * replaced in place when the same check runs again on the same commit. It is the orchestrator's,
 * not the agent's.
 *
 * `name`, `branch`, `summary` and the rest come from whoever runs the CI: untrusted text, drawn as
 * text. The link is drawn only when it is an absolute http(s) URL, checked here again.
 */
export function CiStep({ data }: { data: CiContent }) {
  const { icon: ConclusionIcon, tone } = lookOf(data);
  const label = conclusionLabel(data.conclusion);
  const name = truncate(data.name, NAME_PREVIEW);
  const branch = data.branch ? truncate(data.branch, NAME_PREVIEW) : undefined;
  const href = safeHttpUrl(data.url);
  return (
    <StepRow
      state={data.passed ? "done" : "failed"}
      icon={CircleDotIcon}
      aria-label={`CI: ${name.text}, ${label.toLowerCase()}`}
      data-slot="ci-card"
      data-conclusion={data.conclusion}
      data-passed={data.passed ? "true" : "false"}
      label={
        <>
          <span className="text-muted-foreground">CI</span>
          <span
            data-slot="ci-name"
            title={name.cut ? data.name : undefined}
            className="font-medium text-foreground [overflow-wrap:anywhere]"
          >
            {name.text}
          </span>
          <Badge variant="secondary" className={cn("h-5 gap-1 font-medium", tone)}>
            <ConclusionIcon aria-hidden="true" />
            {label}
          </Badge>
          {data.actor?.type === "agent" ? <ActorLabel actor={data.actor} /> : null}
        </>
      }
    >
      <p className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
        <code title={data.sha} className="rounded bg-muted px-1 py-px font-mono">
          {shortCommit(data.shortSha)}
        </code>
        {branch ? (
          <span data-slot="ci-branch" title={branch.cut ? data.branch : undefined}>
            <GitBranchIcon aria-hidden="true" className="mr-0.5 inline size-3 align-[-0.125em]" />
            <span className="sr-only">Branch </span>
            <span className="[overflow-wrap:anywhere]">{branch.text}</span>
          </span>
        ) : null}
        <span data-slot="ci-source" className="[overflow-wrap:anywhere]">
          {providerLabel(data.provider)} · {truncate(data.repository, NAME_PREVIEW).text}
        </span>
      </p>
      {data.summary ? (
        <p
          data-slot="ci-summary"
          className="text-[0.8125rem] text-foreground/85 [overflow-wrap:anywhere]"
        >
          <ExpandableText text={data.summary} limit={SUMMARY_PREVIEW} />
        </p>
      ) : null}
      {href ? (
        <a
          href={href}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1 self-start rounded-sm text-xs font-medium"
        >
          View run
          <ExternalLinkIcon aria-hidden="true" className="size-3" />
          <span className="sr-only"> (opens in a new tab)</span>
        </a>
      ) : null}
    </StepRow>
  );
}
