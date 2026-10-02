import {
  ACTIVITY,
  ACTOR_PART,
  activityPartName,
  PURPOSE_PART,
  type TextPurpose,
} from "@/features/chat/lib/agui/vymalo";
import {
  buildTurnSteps,
  type StepMessage,
  type TurnSteps,
  type TurnView,
} from "@/features/chat/lib/step-tree";

/*
 * Test support for the step tree: turns built the way the app builds them, from the parts the
 * runtime would hold, so the components are tried on what `buildTurnSteps` really makes.
 */

export const AT = (s: number): string =>
  `2027-01-15T08:${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}Z`;

export const part = (type: string, value: Record<string, unknown>) => ({
  type: "data",
  name: activityPartName(type),
  data: value,
});

export const actorPart = (name = "coder", revision?: string) => ({
  type: "data",
  name: ACTOR_PART,
  data: { type: "agent", name, ...(revision ? { revision } : {}) },
});

/** One report of a step, as `vymalo.step` says it. */
export const stepPart = (
  id: string,
  state: string,
  extra: Record<string, unknown> = {},
  at = 1,
  path: string[] = [],
) =>
  part(ACTIVITY.step, {
    id,
    path,
    kind: "tool",
    label: id,
    state,
    startedAt: AT(at),
    at: AT(at),
    ...extra,
  });

export const statusPart = (status: string, detail?: string, at = 1) =>
  part(ACTIVITY.status, { status, ...(detail ? { detail } : {}), at: AT(at) });

export const textPart = (text: string) => ({ type: "text", text });

/**
 * Words the log marked (ADR 0031) as the runtime holds them: the marker part, then the text. With
 * no purpose it is the bare text, as an unmarked message is.
 */
export const saidPart = (text: string, purpose?: TextPurpose) =>
  purpose
    ? [{ type: "data", name: PURPOSE_PART, data: { purpose } }, textPart(text)]
    : [textPart(text)];

let counter = 0;
export function assistant(
  content: StepMessage["content"],
  status: StepMessage["status"] = { type: "complete" },
  id = `turn-${++counter}`,
): StepMessage {
  return { id, role: "assistant", content, status };
}

export const DONE_VIEW: TurnView = { state: "done", waiting: false, agentId: "coder" };
export const RUNNING_VIEW: TurnView = { state: "working", waiting: false, agentId: "coder" };

/** The turns of a thread made of these messages. */
export const turnsOf = (messages: StepMessage[], view: TurnView = DONE_VIEW): TurnSteps[] =>
  buildTurnSteps(messages, view);

/**
 * A turn in which the agent delegated to OpenCode: `children` steps under it, the one at
 * `failAt` (when given) a command that failed.
 */
export function openCodeTurn(
  children: number,
  options: {
    id?: string;
    failAt?: number;
    running?: boolean;
    name?: string;
    revision?: string;
  } = {},
): StepMessage {
  const { id, failAt, running = false, name = "coder", revision } = options;
  const parts: StepMessage["content"][number][] = [
    actorPart(name, revision),
    statusPart("working", undefined, 1),
    stepPart(
      "T/oc",
      running ? "running" : "completed",
      { kind: "subagent", label: "OpenCode", icon: "agent" },
      2,
    ),
  ];
  for (let i = 0; i < children; i++) {
    const failed = i === failAt;
    parts.push(
      stepPart(
        `T/c${i}`,
        failed ? "failed" : "completed",
        failed
          ? { kind: "command", label: `npm test ${i}`, icon: "execute", detail: "1 failed" }
          : { kind: "tool", label: `step ${i}`, icon: "read" },
        3 + i,
        ["T/oc"],
      ),
    );
  }
  return assistant(parts, running ? { type: "running" } : { type: "complete" }, id);
}
