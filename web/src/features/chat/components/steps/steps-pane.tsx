"use client";

import { ChevronRightIcon } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { formatDuration, pathTo, type TurnSteps } from "@/features/chat/lib/step-tree";
import { EmptyPanel } from "@/features/panel/components/empty-panel";
import { cn } from "@/lib/utils";
import { type ExpansionState, withFocused, withPathOpen } from "./expansion";
import { FailedChip, StepLevel, type TreeScope } from "./step-node";
import { TurnGlyph, turnGlyph } from "./turn-glyph";

export type StepsFocus = { turnId: string; key: number; stepId?: string };

const FLASH_MS = 1500;

const isLiveTurn = (t: TurnSteps): boolean => t.state === "running" || t.state === "verifying";

const capitalise = (name: string): string => name.charAt(0).toUpperCase() + name.slice(1);

/**
 * The turns the pane lists: the ones that did something or said something while they worked (a
 * note, ADR 0031), and the one that is running. A turn that is its answer only has nothing to show.
 */
export const listedTurns = (turns: readonly TurnSteps[]): TurnSteps[] =>
  turns.filter((t) => t.summary.total > 0 || t.summary.notes > 0 || isLiveTurn(t));

function TurnSection({
  turn,
  open,
  flashing,
  scope,
  onToggle,
}: {
  turn: TurnSteps;
  open: boolean;
  flashing: boolean;
  scope: TreeScope;
  onToggle(): void;
}) {
  const id = useId();
  const headingId = `${id}-h`;
  const bodyId = `${id}-b`;
  const [agent, ...gate] = turn.roots;
  const nodes = [...(agent?.children ?? []), ...gate];
  const took =
    turn.summary.durationMs !== undefined ? formatDuration(turn.summary.durationMs) : undefined;
  return (
    <section
      aria-labelledby={headingId}
      data-slot="turn-section"
      data-turn={turn.turnId}
      data-state={turn.state}
      className="flex flex-col"
    >
      <h3
        id={headingId}
        tabIndex={-1}
        data-slot="turn-heading"
        className={cn(
          "rounded-lg outline-none motion-safe:transition-colors motion-safe:duration-500 focus-visible:ring-3 focus-visible:ring-ring/50",
          flashing && "bg-brand/10",
        )}
      >
        <button
          type="button"
          aria-expanded={open}
          aria-controls={open ? bodyId : undefined}
          onClick={onToggle}
          className="flex min-h-9 w-full cursor-pointer items-center gap-2 rounded-lg px-2 text-start text-sm font-medium hover:bg-muted/60 focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
        >
          <ChevronRightIcon
            aria-hidden="true"
            className={cn(
              "size-3.5 shrink-0 text-muted-foreground motion-safe:transition-transform",
              open && "rotate-90",
            )}
          />
          <span className="min-w-0 truncate">
            Turn {turn.number} · <span>{capitalise(turn.agent.name)}</span>
            {took ? ` · ${took}` : ""}
          </span>
          <TurnGlyph icon={turnGlyph(turn.state)} className="ms-auto" />
          {turn.summary.failed > 0 ? <FailedChip count={turn.summary.failed} /> : null}
        </button>
      </h3>
      {open ? (
        <div id={bodyId} className="pt-2 ps-3 pb-2">
          <StepLevel nodes={nodes} scope={scope} label={`Steps of turn ${turn.number}`} />
        </div>
      ) : null}
    </section>
  );
}

/**
 * The agents' work, turn by turn, oldest first: one section per turn with a header that opens it
 * (the turn the shell asked to show, else the live one while the thread runs, else the last), and
 * inside it the steps, where each level shows one line per step and each click on a step with
 * children shows a little more. Prop-driven: the turns, what the shell asked to focus, whether the
 * thread has an open run, and the expansion state that the shell holds above the panel.
 */
export function StepsPane({
  turns,
  focus,
  live,
  expanded,
  onExpandedChange,
}: {
  turns: readonly TurnSteps[];
  focus: StepsFocus | null;
  live: boolean;
  expanded: ExpansionState;
  onExpandedChange(next: ExpansionState): void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const listed = listedTurns(turns);

  const liveTurn = live ? listed.findLast(isLiveTurn) : undefined;
  const preferred = liveTurn?.turnId ?? listed.at(-1)?.turnId;
  const pending = focus !== null && focus.key !== expanded.seenKey ? focus : null;
  const isOpen = (turnId: string): boolean =>
    expanded.turns.has(turnId) ||
    pending?.turnId === turnId ||
    (!expanded.picked && turnId === preferred);

  const toggle = (turnId: string) => {
    const next = new Set(listed.filter((t) => isOpen(t.turnId)).map((t) => t.turnId));
    if (next.has(turnId)) next.delete(turnId);
    else next.add(turnId);
    onExpandedChange({ ...expanded, turns: next, picked: true });
  };

  // a request to show a turn: open it, scroll to it, put the focus on its header, mark it for a
  // moment. Acting on it changes what is pending, so what is scheduled here is cleared on unmount
  // and not when this effect runs again
  const latest = useRef({ expanded, onExpandedChange });
  latest.current = { expanded, onExpandedChange };
  const scheduled = useRef<{ frame?: number; flash?: ReturnType<typeof setTimeout> }>({});
  useEffect(() => {
    const handles = scheduled.current;
    return () => {
      if (handles.frame !== undefined) cancelAnimationFrame(handles.frame);
      clearTimeout(handles.flash);
    };
  }, []);
  const key = pending?.key;
  const wanted = pending?.turnId;
  const wantedStep = pending?.stepId;
  const turnsRef = useRef(turns);
  turnsRef.current = turns;
  useEffect(() => {
    if (key === undefined || wanted === undefined) return;
    const section = [...(root.current?.querySelectorAll<HTMLElement>("[data-turn]") ?? [])].find(
      (el) => el.dataset.turn === wanted,
    );
    // a step of the turn: the way to it opens, and it opens (its input and output, or its steps)
    const turn = wantedStep ? turnsRef.current.find((t) => t.turnId === wanted) : undefined;
    const path = turn && wantedStep ? pathTo(turn, wantedStep) : [];
    const opened = withFocused(latest.current.expanded, wanted, key);
    latest.current.onExpandedChange(path.length > 0 ? withPathOpen(opened, wanted, path) : opened);
    if (!section) return;
    section.scrollIntoView({ block: "start" });
    // a frame later: a sheet's own focus handling runs as it opens, and this has to be last
    const handles = scheduled.current;
    if (handles.frame !== undefined) cancelAnimationFrame(handles.frame);
    const rowOf = (): HTMLElement | undefined =>
      path.length > 0
        ? [...section.querySelectorAll<HTMLElement>("[data-step]")].find(
            (el) => el.dataset.step === wantedStep,
          )
        : undefined;
    // the step's own row exists once what the click above opened has been drawn: look for it for a
    // few frames, then settle for the turn's header
    const focusTarget = (framesLeft: number) => {
      const row = rowOf();
      if (path.length > 0 && !row && framesLeft > 0) {
        handles.frame = requestAnimationFrame(() => focusTarget(framesLeft - 1));
        return;
      }
      const target =
        row?.querySelector<HTMLElement>('[data-slot="step-toggle"]') ??
        section.querySelector<HTMLElement>('[data-slot="turn-heading"]');
      row?.scrollIntoView({ block: "nearest" });
      target?.focus({ preventScroll: true });
    };
    handles.frame = requestAnimationFrame(() => focusTarget(5));
    setFlash(wanted);
    clearTimeout(handles.flash);
    handles.flash = setTimeout(() => setFlash(null), FLASH_MS);
  }, [key, wanted, wantedStep]);

  // while the thread runs and the person has not chosen a turn, keep the step it is on in view
  const [followed, setFollowed] = useState(true);
  const following = live && !expanded.picked && followed;
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs for every new frame of the turns
  useEffect(() => {
    if (!following || !liveTurn) return;
    const section = [...(root.current?.querySelectorAll<HTMLElement>("[data-turn]") ?? [])].find(
      (el) => el.dataset.turn === liveTurn.turnId,
    );
    const running = section?.querySelectorAll<HTMLElement>('[data-state="live"]');
    running?.item(running.length - 1)?.scrollIntoView({ block: "nearest" });
  }, [following, liveTurn, turns]);

  if (listed.length === 0) {
    return (
      <EmptyPanel title="What the agents do shows up here">
        Each turn of an agent lists its steps here, with the commands and sub-agents under them.
      </EmptyPanel>
    );
  }

  return (
    <div
      ref={root}
      data-slot="steps-pane"
      // the person scrolling is the person reading: stop following the live step
      onWheel={() => setFollowed(false)}
      onTouchMove={() => setFollowed(false)}
      className="flex flex-col gap-1 p-2"
    >
      {listed.map((turn) => (
        <TurnSection
          key={turn.turnId}
          turn={turn}
          open={isOpen(turn.turnId)}
          flashing={flash === turn.turnId}
          scope={{ turnId: turn.turnId, expanded, onExpandedChange }}
          onToggle={() => toggle(turn.turnId)}
        />
      ))}
    </div>
  );
}
