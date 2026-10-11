"use client";

import {
  CheckCircle2Icon,
  CircleDashedIcon,
  GitBranchIcon,
  GitMergeIcon,
  GitPullRequestClosedIcon,
  GitPullRequestDraftIcon,
  GitPullRequestIcon,
  XCircleIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import { Badge } from "@/components/ui/badge";
import { hostOf } from "@/features/chat/lib/a2ui/cards";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import { plainName } from "@/features/chat/lib/files";
import { cn } from "@/lib/utils";

/*
 * The `PullRequest` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): a pull request as the
 * agent reports it: its repository and number, its title, its state in words and with an icon, its branch, its
 * checks as counts, and a link. The link is an http(s) URL checked by the validator and again here
 * (`safeHttpUrl`), a plain `<a>` that opens in a new tab. Nothing is fetched for it: no status, no preview.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const count = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isInteger(v) && v >= 0 ? v : undefined;

const STATES = {
  open: { label: "Open", Icon: GitPullRequestIcon, tone: "bg-success/15 text-success" },
  merged: { label: "Merged", Icon: GitMergeIcon, tone: "bg-verifying/15 text-verifying" },
  closed: {
    label: "Closed",
    Icon: GitPullRequestClosedIcon,
    tone: "bg-destructive/15 text-destructive",
  },
  draft: { label: "Draft", Icon: GitPullRequestDraftIcon, tone: "bg-muted text-muted-foreground" },
} as const;

type StateName = keyof typeof STATES;
const isState = (v: unknown): v is StateName => typeof v === "string" && v in STATES;

/** The PullRequest of a surface; the props are those of the lowered component (`prepare.ts`). */
export function PullRequestView(props: Rec): ReactNode {
  const repo = plainName(str(props.repo) ?? "", 200);
  const number = count(props.number);
  const title = plainName(str(props.title) ?? "", 300);
  const branch = plainName(str(props.branch) ?? "", 200);
  const state: StateName = isState(props.state) ? props.state : "open";
  const { label, Icon, tone } = STATES[state];
  const href = safeHttpUrl(props.url);
  const checks =
    props.checks && typeof props.checks === "object" ? (props.checks as Rec) : undefined;
  const passed = count(checks?.passed);
  const failed = count(checks?.failed);
  const pending = count(checks?.pending);
  return (
    <article
      data-slot="pull-request"
      data-state={state}
      className="flex min-w-0 flex-col gap-2 rounded-xl border bg-card p-3.5"
    >
      <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
        {repo}
        {number !== undefined ? ` #${number}` : ""}
      </p>
      {/* biome-ignore lint/a11y/useSemanticElements: the level is the surface's, not the page's */}
      <p
        role="heading"
        aria-level={3}
        className="text-sm leading-snug font-medium [overflow-wrap:anywhere]"
      >
        {href ? (
          <a
            href={href}
            target="_blank"
            rel="noopener noreferrer"
            className="text-brand underline-offset-2 hover:underline focus-visible:rounded-sm focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
          >
            {title}
            <span className="sr-only"> (opens in a new tab)</span>
          </a>
        ) : (
          title
        )}
      </p>
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 text-xs">
        <Badge
          variant="outline"
          data-slot="pull-request-state"
          className={cn("h-auto gap-1 border-transparent", tone)}
        >
          <Icon aria-hidden="true" className="size-3" />
          {label}
        </Badge>
        {href ? (
          <span
            data-slot="pull-request-host"
            className="min-w-0 text-muted-foreground [overflow-wrap:anywhere]"
          >
            {hostOf(href)}
          </span>
        ) : null}
        {branch ? (
          <span className="inline-flex min-w-0 items-center gap-1 text-muted-foreground">
            <GitBranchIcon aria-hidden="true" className="size-3 shrink-0" />
            <span className="sr-only">Branch: </span>
            <span className="font-mono [overflow-wrap:anywhere]">{branch}</span>
          </span>
        ) : null}
      </div>
      {passed !== undefined && failed !== undefined && pending !== undefined ? (
        <ul
          aria-label="Checks"
          className="m-0 flex list-none flex-wrap gap-x-3 gap-y-1 p-0 text-xs"
        >
          <li className="inline-flex items-center gap-1">
            <CheckCircle2Icon aria-hidden="true" className="size-3.5 text-success" />
            {`${passed} passed`}
          </li>
          <li className="inline-flex items-center gap-1">
            <XCircleIcon aria-hidden="true" className="size-3.5 text-destructive" />
            {`${failed} failed`}
          </li>
          <li className="inline-flex items-center gap-1 text-muted-foreground">
            <CircleDashedIcon aria-hidden="true" className="size-3.5" />
            {`${pending} pending`}
          </li>
        </ul>
      ) : null}
    </article>
  );
}
