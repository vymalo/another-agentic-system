"use client";

import { useAuiState } from "@assistant-ui/react";
import { ArrowUpRightIcon, FileTextIcon, GitPullRequestIcon } from "lucide-react";
import { useState } from "react";
import {
  ACTIVITY,
  type ArtifactContent,
  activityPartName,
  parseArtifact,
} from "@/features/chat/lib/agui/vymalo";
import { safeLinkHref } from "@/features/chat/lib/artifact";
import { truncate } from "@/features/chat/lib/findings";
import { isCardArtifact, type PullRequestView, pullRequestOf } from "@/features/chat/lib/steps";
import { BranchChip } from "../steps/step-items";

/** Lines of a file's text shown before "Show all". */
const FILE_LINES = 8;
/** A note under a pull request longer than this is cut. */
const NOTE_PREVIEW = 280;

/**
 * A pull request the agent opened: its title, where it is, its branch, and a button to open it.
 * The link is the https URL the projection checked (or a recognised pull request URL).
 */
export function PullRequestCard({ pr }: { pr: PullRequestView }) {
  const note = pr.note ? truncate(pr.note.trim(), NOTE_PREVIEW).text : undefined;
  return (
    <div
      data-slot="pull-request-card"
      className="flex w-full max-w-xl items-start gap-3 rounded-xl border bg-card p-3.5 sm:p-4"
    >
      <span
        aria-hidden="true"
        className="flex size-9 shrink-0 items-center justify-center rounded-full bg-success/10 text-success"
      >
        <GitPullRequestIcon className="size-4.5" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <p className="text-[0.9375rem] leading-snug font-medium text-foreground [overflow-wrap:anywhere]">
          {pr.title ?? (pr.number !== undefined ? `Pull request #${pr.number}` : "Pull request")}
        </p>
        <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
          <span className="[overflow-wrap:anywhere]">{pr.label}</span>
          {pr.branch ? <BranchChip branch={pr.branch} /> : null}
        </p>
        {note ? (
          <p className="pt-1 text-[0.8125rem] whitespace-pre-wrap text-foreground/85 [overflow-wrap:anywhere]">
            {note}
          </p>
        ) : null}
        <a
          href={pr.href}
          target="_blank"
          rel="noopener noreferrer"
          className="mt-2 inline-flex h-8 items-center gap-1.5 self-start rounded-full bg-primary px-3.5 text-[0.8125rem] font-medium text-primary-foreground no-underline transition-colors hover:bg-primary/90 focus-visible:outline-offset-2"
        >
          View pull request <span className="sr-only">{pr.label} (opens in a new tab)</span>
          <ArrowUpRightIcon aria-hidden="true" className="size-3.5" />
        </a>
      </div>
    </div>
  );
}

/** A file the agent shared: its name and type, its text (folded when long) and its link. */
export function FileCard({ data }: { data: ArtifactContent }) {
  const [open, setOpen] = useState(false);
  const href = safeLinkHref(data.uri);
  const lines = data.text?.split("\n") ?? [];
  const long = lines.length > FILE_LINES || (data.text?.length ?? 0) > 1200;
  const shown =
    open || !long ? data.text : `${lines.slice(0, FILE_LINES).join("\n").slice(0, 1200)}…`;
  return (
    <div
      data-slot="file-card"
      className="flex w-full max-w-xl flex-col gap-2 rounded-xl border bg-card p-3.5 sm:p-4"
    >
      <div className="flex min-w-0 items-center gap-3">
        <span
          aria-hidden="true"
          className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"
        >
          <FileTextIcon className="size-4.5" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{data.name}</p>
          {data.mimeType ? (
            <p className="truncate text-xs text-muted-foreground">{data.mimeType}</p>
          ) : null}
        </div>
        {href ? (
          <a
            href={href}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex shrink-0 items-center gap-1 text-[0.8125rem] font-medium"
          >
            Open {data.name} <span className="sr-only">(opens in a new tab)</span>
            <ArrowUpRightIcon aria-hidden="true" className="size-3.5" />
          </a>
        ) : null}
      </div>
      {shown ? (
        <pre
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
          tabIndex={0}
          className="max-h-96 overflow-auto rounded-lg bg-muted/60 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere]"
        >
          {shown}
        </pre>
      ) : null}
      {long ? (
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
          className="cursor-pointer self-start rounded-sm text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
        >
          {open ? "Show less" : "Show all"}
        </button>
      ) : null}
    </div>
  );
}

/**
 * The cards of a turn, after the agent's words: every pull request and file it shared, in order.
 * Their steps are in the step list; the card is what a person acts on.
 */
export function TurnCards() {
  const parts = useAuiState((s) => s.message.content) as readonly {
    type: string;
    name?: string;
    data?: unknown;
  }[];
  const artifacts = parts.flatMap((p) => {
    if (p.type !== "data" || p.name !== activityPartName(ACTIVITY.artifact)) return [];
    const a = parseArtifact(p.data);
    return a && isCardArtifact(a) ? [a] : [];
  });
  if (artifacts.length === 0) return null;
  return (
    <div data-slot="turn-cards" className="flex flex-col gap-2">
      {artifacts.map((a, n) => {
        const pr = pullRequestOf(a);
        // biome-ignore lint/suspicious/noArrayIndexKey: artifacts only append; the order is the key
        return pr ? <PullRequestCard key={n} pr={pr} /> : <FileCard key={n} data={a} />;
      })}
    </div>
  );
}
