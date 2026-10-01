"use client";

import { useAuiState } from "@assistant-ui/react";
import { useState } from "react";
import {
  ACTIVITY,
  activityPartName,
  parseAction,
  parseArtifact,
  parseCheck,
  parseCi,
  parseRework,
  parseStatus,
  parseStep,
} from "@/features/chat/lib/agui/vymalo";
import { stepNode } from "@/features/chat/lib/step-tree";
import { drawsStep } from "@/features/chat/lib/steps";
import { CheckStep } from "./check-step";
import { CiStep } from "./ci-step";
import { NO_EXPANSION } from "./expansion";
import { ActionStep, ArtifactStep, ReworkStep, StatusStep } from "./step-items";
import { StepNodeView, type TreeScope } from "./step-node";

/** Past this many steps the earliest fold behind a control; the latest stay in view. */
export const STEPS_SHOWN = 30;
/** How many of the latest steps a folded list keeps. */
const STEPS_KEPT = 20;

type Part = { type: string; name?: string; data?: unknown };

/** A step of steps/v1 in this flat list has no tree to open: the side panel is where it nests. */
const FLAT: TreeScope = { turnId: "flat", expanded: NO_EXPANSION, onExpandedChange: () => {} };

/** One step: the part drawn by the renderer of its activity. `live` marks the agent's current one. */
function Step({ part, live, running }: { part: Part; live: boolean; running: boolean }) {
  switch (part.name) {
    case activityPartName(ACTIVITY.status): {
      const status = parseStatus(part.data);
      return status ? <StatusStep data={status} live={live} /> : null;
    }
    case activityPartName(ACTIVITY.artifact): {
      const artifact = parseArtifact(part.data);
      return artifact ? <ArtifactStep data={artifact} live={live} /> : null;
    }
    case activityPartName(ACTIVITY.check): {
      const check = parseCheck(part.data);
      return check ? <CheckStep data={check} waiting={running} /> : null;
    }
    case activityPartName(ACTIVITY.ci): {
      const ci = parseCi(part.data);
      return ci ? <CiStep data={ci} /> : null;
    }
    case activityPartName(ACTIVITY.rework): {
      const rework = parseRework(part.data);
      return rework ? <ReworkStep data={rework} /> : null;
    }
    case activityPartName(ACTIVITY.action): {
      const action = parseAction(part.data);
      return action ? <ActionStep data={action} /> : null;
    }
    case activityPartName(ACTIVITY.step): {
      const step = parseStep(part.data);
      return step ? <StepNodeView node={stepNode(step)} scope={FLAT} /> : null;
    }
    default:
      return null;
  }
}

/**
 * The agent's steps, always in view (web/DESIGN.md): one compact line per activity of the turn,
 * in order. While the turn runs, the last line is the step the agent is on and spins; a check
 * that waits for its answer spins on its own. A long list folds its earliest steps.
 */
export function StepList({ indices, last }: { indices: readonly number[]; last: boolean }) {
  const parts = useAuiState((s) => s.message.content) as readonly Part[];
  const running = useAuiState((s) => s.message.status?.type === "running");
  const [all, setAll] = useState(false);

  const drawn = indices.flatMap((i) => {
    const part = parts[i];
    return part && drawsStep(part) ? [{ i, part }] : [];
  });
  if (drawn.length === 0) return null;
  const hidden = all || drawn.length <= STEPS_SHOWN ? 0 : drawn.length - STEPS_KEPT;
  const shown = drawn.slice(hidden);
  const lastIndex = drawn.at(-1)?.i;

  return (
    <div data-slot="steps" className="flex flex-col gap-1">
      {hidden > 0 ? (
        <button
          type="button"
          onClick={() => setAll(true)}
          className="ml-8 cursor-pointer self-start rounded-sm text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
        >
          Show {hidden} earlier {hidden === 1 ? "step" : "steps"}
        </button>
      ) : null}
      <ol aria-label="Steps" className="flex flex-col">
        {shown.map(({ i, part }) => (
          <Step key={i} part={part} running={running} live={running && last && i === lastIndex} />
        ))}
      </ol>
    </div>
  );
}
