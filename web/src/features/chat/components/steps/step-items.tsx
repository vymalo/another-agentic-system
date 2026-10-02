import {
  BanIcon,
  ClockIcon,
  FileTextIcon,
  FlaskConicalIcon,
  GitBranchIcon,
  GitPullRequestIcon,
  KeyRoundIcon,
  MessageSquareTextIcon,
  MousePointerClickIcon,
  PlayIcon,
  RotateCcwIcon,
} from "lucide-react";
import { useId, useState } from "react";
import type {
  ActionContent,
  ArtifactContent,
  ReworkContent,
  StatusContent,
} from "@/features/chat/lib/agui/vymalo";
import { pluralFindings, shortCommit } from "@/features/chat/lib/findings";
import {
  checksPayload,
  pullRequestOf,
  reworkLabel,
  shortRepository,
} from "@/features/chat/lib/steps";
import { cn } from "@/lib/utils";
import { ActorLabel } from "../actor-label";
import { ExpandableText } from "../parts/expandable-text";
import { FindingsList } from "../parts/findings-list";
import { StepRow, type StepState } from "./step-row";

/** A step's words longer than this are cut, with a control to read the rest. */
export const STEP_PREVIEW = 160;

/** `live` turns a finished-looking step into the one the agent is on. */
type Live = { live?: boolean };

const doneOr = (live: boolean | undefined, state: StepState = "done"): StepState =>
  live ? "live" : state;

/** A branch name in a small monospace chip, with its icon. */
export function BranchChip({ branch }: { branch: string }) {
  return (
    <span
      data-slot="branch-chip"
      className="inline-flex max-w-full min-w-0 items-center gap-1 rounded-md bg-muted px-1.5 py-px font-mono text-xs text-foreground"
    >
      <GitBranchIcon aria-hidden="true" className="size-3 shrink-0 text-muted-foreground" />
      <span className="truncate">{branch}</span>
    </span>
  );
}

/**
 * An agent status that is a step: working (with the agent's words), queued, stopped, sign-in. A
 * `working` status whose words are a command (`$ …`) is a command step of the tree, not this.
 */
export function StatusStep({ data, live }: { data: StatusContent } & Live) {
  switch (data.status) {
    case "working": {
      const detail = data.detail?.trim();
      if (!detail) return <StepRow state={doneOr(live)} icon={PlayIcon} label="Started working" />;
      return (
        <StepRow
          state={doneOr(live)}
          label={
            <span className="min-w-0 [overflow-wrap:anywhere]">
              <ExpandableText text={detail} limit={STEP_PREVIEW} />
            </span>
          }
        />
      );
    }
    case "submitted":
      return <StepRow state={doneOr(live)} icon={ClockIcon} label="Queued" />;
    case "auth_required":
      return (
        <StepRow
          state="warning"
          icon={KeyRoundIcon}
          label={data.detail ? `Needs you to sign in: ${data.detail}` : "Needs you to sign in"}
        />
      );
    case "canceled":
      return <StepRow state="muted" icon={BanIcon} label="Stopped" />;
    default:
      return null;
  }
}

/** An artifact as a step: a push, the agent's checks, a pull request, a file. */
export function ArtifactStep({ data, live }: { data: ArtifactContent } & Live) {
  switch (data.kind) {
    case "branch":
      return (
        <StepRow
          state={doneOr(live)}
          icon={GitBranchIcon}
          label={
            <>
              <span>Pushed</span>
              <BranchChip branch={data.branch ?? data.name} />
              {data.repository || data.shortSha ? (
                <span className="text-xs text-muted-foreground">
                  {data.repository ? `to ${shortRepository(data.repository)}` : ""}
                  {data.repository && data.shortSha ? " · " : ""}
                  {data.shortSha ? (
                    <code title={data.sha} className="font-mono">
                      {shortCommit(data.shortSha)}
                    </code>
                  ) : null}
                </span>
              ) : null}
            </>
          }
        />
      );
    case "checks": {
      const { summary, findings } = checksPayload(data.text);
      const passed = data.passed === true;
      return (
        <StepRow
          state={passed ? doneOr(live) : "failed"}
          icon={FlaskConicalIcon}
          data-slot="checks-step"
          label={
            <>
              <span>{passed ? "Checks passed" : "Checks failed"}</span>
              {summary ? <span className="text-muted-foreground">· {summary}</span> : null}
            </>
          }
        >
          {!passed && findings.length > 0 ? <FindingsDisclosure findings={findings} /> : null}
        </StepRow>
      );
    }
    case "pull_request":
    case "file": {
      const pr = pullRequestOf(data);
      if (pr) {
        return (
          <StepRow
            state={doneOr(live)}
            icon={GitPullRequestIcon}
            label={
              pr.number !== undefined
                ? `Opened pull request #${pr.number}`
                : "Opened a pull request"
            }
          />
        );
      }
      return <StepRow state={doneOr(live)} icon={FileTextIcon} label={`Shared ${data.name}`} />;
    }
    default:
      return null;
  }
}

/** Findings folded behind a small disclosure, open on request: the step list stays compact. */
export function FindingsDisclosure({ findings }: { findings: readonly string[] }) {
  return (
    <details
      data-slot="findings-disclosure"
      className="group/findings max-w-xl rounded-lg border bg-card px-3 py-1.5 text-[0.8125rem] open:pb-3"
    >
      <summary className="cursor-pointer py-0.5 text-xs font-medium text-muted-foreground marker:text-muted-foreground hover:text-foreground">
        Findings ({findings.length})
      </summary>
      <div className="pt-2">
        <FindingsList findings={findings} heading={false} />
      </div>
    </details>
  );
}

/**
 * The gate failed and the agent is sent back (ADR 0018): "Checks failed — trying again (2/3)" and
 * how many findings it took back; the findings themselves are on the check steps above.
 */
export function ReworkStep({ data }: { data: ReworkContent }) {
  const count = data.findings.reduce((sum, f) => sum + f.findings.length, 0);
  return (
    <StepRow
      state="warning"
      icon={RotateCcwIcon}
      data-slot="rework-step"
      data-attempt={data.attempt}
      label={
        <>
          <span className="font-medium text-foreground">{reworkLabel(data)}</span>
          {count > 0 ? (
            <span className="text-muted-foreground">· {pluralFindings(count)}</span>
          ) : null}
        </>
      }
    />
  );
}

/** What someone did on an A2UI surface (ADR 0013): "Chose Go", and who, when the log says. */
export function ActionStep({ data }: { data: ActionContent }) {
  return (
    <StepRow
      state="done"
      icon={MousePointerClickIcon}
      data-slot="action-step"
      label={
        <>
          <span className="[overflow-wrap:anywhere]">
            Chose <strong className="font-medium text-foreground">{data.name}</strong>
            {data.sourceComponentId && data.sourceComponentId !== data.name
              ? ` (${data.sourceComponentId})`
              : ""}
          </span>
          <ActorLabel actor={data.actor} />
        </>
      }
    />
  );
}

/** A note longer than this (or of more lines than `NOTE_LINES`) is clamped, with a control for the rest. */
const NOTE_FOLD = 240;
const NOTE_LINES = 4;

/**
 * What the agent said while it worked (ADR 0031), as a row among its steps, in the place in time
 * it was said. The chat keeps one answer per turn; this is where the rest is. The words are the
 * agent's own: a text node, never markup. They are whole in the page for a screen reader even when
 * a long one is clamped to three lines for the eye, and "Working note" is said before them so the
 * row reads as what it is.
 */
export function NoteStep({ id, text }: { id: string; text: string }) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const long = text.length > NOTE_FOLD || text.split("\n").length > NOTE_LINES;
  return (
    <StepRow
      state="done"
      icon={MessageSquareTextIcon}
      data-slot="step"
      data-kind="note"
      data-step={id}
      label={
        <span
          id={bodyId}
          data-slot="note-text"
          className={cn(
            "block min-w-0 flex-1 basis-full whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]",
            long && !open && "line-clamp-3",
          )}
        >
          <span className="sr-only">Working note: </span>
          {text}
        </span>
      }
    >
      {long ? (
        <button
          type="button"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen((o) => !o)}
          className="cursor-pointer self-start rounded-sm text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
        >
          {open ? "Show less" : "Show more"}
        </button>
      ) : null}
    </StepRow>
  );
}
