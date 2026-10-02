"use client";

import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronRightIcon, CircleXIcon, LoaderCircleIcon } from "lucide-react";
import { useId, useRef } from "react";
import { Badge } from "@/components/ui/badge";
import { inputPreview, toolName } from "@/features/chat/lib/step-label";
import {
  countUnder,
  hasIo,
  nodeDuration,
  plural,
  type StepNode,
  type StepState,
  visibleChildren,
} from "@/features/chat/lib/step-tree";
import { cn } from "@/lib/utils";
import { ExpandableText } from "../parts/expandable-text";
import { CheckStep } from "./check-step";
import { CiStep } from "./ci-step";
import { CommandText } from "./command-text";
import {
  type ExpansionState,
  nodeKey,
  SCROLL_FROM,
  SHOW_MORE,
  showMore,
  shownOf,
  withShown,
} from "./expansion";
import { iconOf } from "./step-icons";
import { StepIo } from "./step-io";
import {
  ActionStep,
  ArtifactStep,
  NoteStep,
  ReworkStep,
  STEP_PREVIEW,
  StatusStep,
} from "./step-items";
import { type RowProps, RowPropsContext, type StepState as RowState, StepRow } from "./step-row";

/** What the tree needs to draw a level: which turn it is in, and what the person has opened. */
export type TreeScope = {
  turnId: string;
  expanded: ExpansionState;
  onExpandedChange(next: ExpansionState): void;
};

const ROW_STATE: Record<StepState, RowState> = {
  running: "live",
  waiting: "waiting",
  completed: "done",
  failed: "failed",
  canceled: "muted",
};

/** What a screen reader hears before a step that is not simply done. */
const STATE_WORD: Record<StepState, string | undefined> = {
  running: "Running",
  waiting: "Waiting",
  completed: undefined,
  failed: "Failed",
  canceled: "Stopped",
};

/** The destructive chip of a level that holds failures: icon and words, colour backs them up. */
export function FailedChip({ count }: { count: number }) {
  return (
    <Badge
      variant="secondary"
      data-slot="failed-chip"
      className="h-5 shrink-0 gap-1 bg-destructive-soft font-medium text-destructive"
    >
      <CircleXIcon aria-hidden="true" />
      {count} failed
    </Badge>
  );
}

/**
 * The failed chip as a button, for the one place it is a control of its own: the chat's line for a
 * turn, where it takes the person to the first step that failed. Its name does not say "message".
 */
export function FailedChipButton({ count, onClick }: { count: number; onClick(): void }) {
  return (
    <button
      type="button"
      data-slot="failed-chip"
      aria-label={`${count} failed. Show the first one in the side panel`}
      onClick={onClick}
      className="inline-flex h-5 shrink-0 cursor-pointer items-center gap-1 rounded-full bg-destructive-soft px-2 text-xs font-medium text-destructive hover:brightness-95 focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none [&_svg]:size-3"
    >
      <CircleXIcon aria-hidden="true" />
      {count} failed
    </button>
  );
}

const COMMAND_WORDS: Record<StepState, string> = {
  running: "Running a command",
  waiting: "Waiting to run a command",
  completed: "Ran a command",
  failed: "Command failed",
  canceled: "Command stopped",
};

/**
 * One step of the tree. The activities of today (a status, an artifact, a check, a CI report, a
 * rework, an action) are leaves drawn by the renderers that always drew them; a step of steps/v1
 * (a sub-agent, a tool, a command, a message) is a row that opens onto its children. `place` is how
 * a virtualised level positions the row (the renderers need not know).
 */
export function StepNodeView({
  node,
  scope,
  place,
}: {
  node: StepNode;
  scope: TreeScope;
  place?: RowProps | undefined;
}) {
  const row = (() => {
    switch (node.kind) {
      case "status":
        return <StatusStep data={node.content} live={node.state === "running"} />;
      case "artifact":
        return <ArtifactStep data={node.content} live={node.state === "running"} />;
      case "check":
        return <CheckStep data={node.content} waiting={node.state === "running"} />;
      case "ci":
        return <CiStep data={node.content} />;
      case "rework":
        return <ReworkStep data={node.content} />;
      case "action":
        return <ActionStep data={node.content} />;
      case "note":
        return <NoteStep id={node.id} text={node.content.text} />;
      case "agent":
        return null;
      default:
        return <TreeStep node={node} scope={scope} />;
    }
  })();
  return <RowPropsContext.Provider value={place ?? null}>{row}</RowPropsContext.Provider>;
}

function TreeStep({ node, scope }: { node: StepNode; scope: TreeScope }) {
  const listId = useId();
  const ioId = `${listId}-io`;
  const key = nodeKey(scope.turnId, node.id);
  const shown = shownOf(scope.expanded, key);
  const counts = countUnder(node);
  const hasChildren = node.children.length > 0;
  const hasDetails = hasIo(node);
  const expandable = hasChildren || hasDetails;
  const open = expandable && shown > 0;
  const Icon = iconOf(node);
  const duration = nodeDuration(node);
  const word = STATE_WORD[node.state];
  const isCommand = node.kind === "command";
  // an MCP tool is "Web search" from the server "search", with what it was asked
  const tool = node.kind === "tool" ? toolName(node.label) : undefined;
  const preview = node.kind === "tool" ? inputPreview(node.input) : undefined;
  const text = isCommand ? COMMAND_WORDS[node.state] : (tool?.title ?? node.label);
  // a step with children lists them; one without opens its input and output
  const toggle = () =>
    scope.onExpandedChange(
      hasChildren
        ? open
          ? withShown(scope.expanded, key, 0)
          : showMore(scope.expanded, key)
        : withShown(scope.expanded, key, open ? 0 : 1),
    );
  const controls = [hasDetails && open ? ioId : null, hasChildren && open ? listId : null]
    .filter(Boolean)
    .join(" ");

  const words = (
    <>
      {word ? <span className="sr-only">{word}: </span> : null}
      <span
        className={cn(
          "min-w-0 truncate",
          // a tool's name is short: it is the quoted argument that gives way on a narrow panel
          tool?.server && "shrink-0",
          isCommand && "text-muted-foreground",
        )}
        title={isCommand || !tool?.server ? text : node.label}
      >
        {text}
      </span>
      {tool?.server ? (
        <span
          data-slot="step-server"
          className="shrink-0 rounded-sm bg-muted px-1 font-mono text-[0.6875rem] leading-4 text-muted-foreground"
        >
          <span className="sr-only">from </span>
          {tool.server}
        </span>
      ) : null}
      {preview ? (
        <span
          data-slot="step-preview"
          className="min-w-0 truncate text-xs text-muted-foreground"
          title={preview}
        >
          {preview}
        </span>
      ) : null}
    </>
  );

  return (
    <StepRow
      state={ROW_STATE[node.state]}
      {...(Icon ? { icon: Icon } : {})}
      data-slot="step"
      data-kind={node.kind}
      data-node-state={node.state}
      data-step={node.id}
      label={
        <>
          {expandable ? (
            <button
              type="button"
              aria-expanded={open}
              aria-controls={open && controls ? controls : undefined}
              onClick={toggle}
              data-slot="step-toggle"
              className="-ms-1 inline-flex max-w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-md px-1 text-start hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
            >
              <ChevronRightIcon
                aria-hidden="true"
                className={cn(
                  "size-3.5 shrink-0 text-muted-foreground motion-safe:transition-transform",
                  open && "rotate-90",
                )}
              />
              {words}
              {hasChildren ? (
                <span className="shrink-0 text-xs text-muted-foreground">
                  · {plural(counts.total, "step")}
                </span>
              ) : null}
            </button>
          ) : (
            <span className="inline-flex max-w-full min-w-0 items-center gap-1.5">{words}</span>
          )}
          {counts.failed > 0 && !open ? <FailedChip count={counts.failed} /> : null}
          {node.state !== "running" && counts.running > 0 ? (
            <LoaderCircleIcon
              aria-hidden="true"
              className="size-3.5 shrink-0 text-brand motion-safe:animate-spin"
            />
          ) : null}
          {counts.running > 0 && node.state !== "running" ? (
            <span className="sr-only">Running</span>
          ) : null}
          {duration ? (
            <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{duration}</span>
          ) : null}
        </>
      }
    >
      {isCommand ? <CommandText command={node.label} /> : null}
      {node.detail ? (
        <p
          data-slot="step-detail"
          className="text-xs text-muted-foreground [overflow-wrap:anywhere]"
        >
          <ExpandableText text={node.detail} limit={STEP_PREVIEW} />
        </p>
      ) : null}
      {open && hasDetails ? (
        <StepIo
          id={ioId}
          input={node.input}
          output={node.output}
          ioDropped={node.ioDropped}
          failed={node.state === "failed"}
          detail={node.detail}
        />
      ) : null}
      {open && hasChildren ? (
        <ChildLevel
          id={listId}
          node={node}
          scope={scope}
          shown={shown}
          label={`Steps of ${node.label}`}
          onMore={() => scope.onExpandedChange(showMore(scope.expanded, key))}
        />
      ) : null}
    </StepRow>
  );
}

/** The latest children of an opened step, the failed ones always, and the control for the rest. */
function ChildLevel({
  id,
  node,
  scope,
  shown,
  label,
  onMore,
}: {
  id: string;
  node: StepNode;
  scope: TreeScope;
  shown: number;
  label: string;
  onMore(): void;
}) {
  const { nodes, hidden } = visibleChildren(node, shown);
  return (
    <div data-slot="step-children" className="flex flex-col gap-1.5">
      {hidden > 0 ? (
        <button
          type="button"
          onClick={onMore}
          aria-label={`Show ${Math.min(SHOW_MORE, hidden)} more steps of ${node.label}`}
          className="cursor-pointer self-start rounded-sm text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
        >
          Show {Math.min(SHOW_MORE, hidden)} more
        </button>
      ) : null}
      <StepLevel id={id} nodes={nodes} scope={scope} label={label} />
    </div>
  );
}

/**
 * A list of steps. Past `SCROLL_FROM` rows it is a scroll box (360 px at most) that draws only the
 * rows in view, so a thousand steps cost what a few dozen do.
 */
export function StepLevel({
  id,
  nodes,
  scope,
  label,
}: {
  id?: string;
  nodes: readonly StepNode[];
  scope: TreeScope;
  label: string;
}) {
  if (nodes.length > SCROLL_FROM) {
    return <VirtualLevel id={id} nodes={nodes} scope={scope} label={label} />;
  }
  return (
    <ol id={id} aria-label={label} className="flex flex-col">
      {nodes.map((node) => (
        <StepNodeView key={node.id} node={node} scope={scope} />
      ))}
    </ol>
  );
}

/** Row heights are measured, because a step may be open onto its own children. */
const ESTIMATED_ROW = 32;

function VirtualLevel({
  id,
  nodes,
  scope,
  label,
}: {
  id: string | undefined;
  nodes: readonly StepNode[];
  scope: TreeScope;
  label: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({
    count: nodes.length,
    getScrollElement: () => box.current,
    estimateSize: () => ESTIMATED_ROW,
    overscan: 8,
    getItemKey: (i) => nodes[i]?.id ?? i,
  });
  return (
    <section
      ref={box}
      aria-label={`${label}, scrolls`}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll box has to be reachable to be scrolled by keyboard
      tabIndex={0}
      data-slot="steps-scroll"
      className="max-h-90 overflow-y-auto overscroll-contain rounded-md focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
    >
      <ol
        id={id}
        aria-label={label}
        style={{ height: virtual.getTotalSize() }}
        className="relative w-full"
      >
        {virtual.getVirtualItems().map((item) => {
          const node = nodes[item.index];
          if (!node) return null;
          return (
            <StepNodeView
              key={item.key}
              node={node}
              scope={scope}
              place={{
                ref: virtual.measureElement,
                style: { transform: `translateY(${item.start}px)` },
                className: "absolute top-0 left-0 w-full",
                "data-index": item.index,
                "aria-setsize": nodes.length,
                "aria-posinset": item.index + 1,
              }}
            />
          );
        })}
      </ol>
    </section>
  );
}
