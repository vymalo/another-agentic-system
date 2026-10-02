import {
  ACTIVITY,
  ACTOR_PART,
  type ActionContent,
  type ArtifactContent,
  activityPartName,
  type CheckContent,
  type CiContent,
  parseAction,
  parseActor,
  parseArtifact,
  parseCheck,
  parseCi,
  parseRework,
  parseStatus,
  parseStep,
  type ReworkContent,
  type StatusContent,
  type StepContent,
  type StepIcon,
  type StepInput,
  type StepOutput,
  type StepState,
} from "@/features/chat/lib/agui/vymalo";
import { conclusionLabel } from "./ci";
import { truncate } from "./findings";
import { toolName } from "./step-label";
import { checkLabel, commandOf, drawsPart, drawsStep, pullRequestOf, reworkLabel } from "./steps";

export type { StepIcon, StepState };

/*
 * The shape of an agent's work, for the panel's Activity tab and the one line each turn keeps in
 * the chat (web/DESIGN.md, "Steps panel"). Pure: the runtime's messages in, one tree per agent turn
 * out. The step parts the messages already hold (`vymalo.step`, and today's activities: statuses,
 * artifacts, checks, CI reports, reworks, actions) are the only source, so the chat's line and the
 * panel never disagree and nothing is fetched.
 */

/** What a part looks like in the runtime's messages (the same shape `panel/lib/sources.ts` reads). */
type PartLike = { type: string; name?: string; text?: string; data?: unknown };

/** The runtime's message status, as far as a turn needs it. */
type StatusLike = { type: string; reason?: string };

export type StepMessage = {
  id: string;
  role: string;
  content: readonly PartLike[];
  status?: StatusLike | undefined;
};

/** What the thread says about itself, from `ThreadView`. */
export type TurnView = {
  state?: string | undefined;
  waiting?: boolean | undefined;
  agentId?: string | null | undefined;
};

type Base = {
  /** The `vymalo.step` id; `part-<message id>-<index>` for the other activities. */
  id: string;
  label: string;
  state: StepState;
  icon?: StepIcon;
  detail?: string;
  /** RFC 3339, from the activity: when the step started and when it last said something. */
  startedAt?: string;
  at?: string;
  /** What the tool was called with, and what it returned (ADR 0030): untrusted, drawn as text. */
  input?: StepInput;
  output?: StepOutput;
  /** The job's record budget had no room for this step's input or output. */
  ioDropped?: true;
  /**
   * A failed `checks` artifact that says what the failed `run_checks` step beside it already said:
   * drawn, but not counted a second time.
   */
  echo?: true;
  /** The runtime part it came from. */
  part?: { messageId: string; index: number };
  children: StepNode[];
};

/**
 * A node of a turn's tree. `agent` is the turn's own root (the panel's turn header stands for it);
 * `subagent`, `tool`, `command` and `message` are steps/v1's kinds; the rest are today's
 * activities as leaves, which carry their parsed `content` for the renderers that draw them.
 */
export type StepNode = Base &
  (
    | { kind: "agent" | "subagent" | "tool" | "command" | "message"; content?: undefined }
    | { kind: "status"; content: StatusContent }
    | { kind: "artifact"; content: ArtifactContent }
    | { kind: "check"; content: CheckContent }
    | { kind: "ci"; content: CiContent }
    | { kind: "rework"; content: ReworkContent }
    | { kind: "action"; content: ActionContent }
  );

export type StepKind = StepNode["kind"];

export type StepSummary = {
  /** Every step of the turn's tree (the turn's own root is not one). */
  total: number;
  running: number;
  failed: number;
  durationMs?: number;
  /** What the turn is on, as a phrase: the deepest running step ("Running npm test"), else the last. */
  current?: string;
};

export type TurnState = StepState | "verifying";

export type TurnSteps = {
  /** The assistant message id: the key the chat (`data-turn-id`) and the panel share. */
  turnId: string;
  /** 1-based position among the thread's agent turns, as `panel/lib/sources.ts` numbers them. */
  number: number;
  agent: { name: string; revision?: string };
  /**
   * The turn's own outcome. `running` only while the thread's run is open and `waiting` while the
   * agent waits for an answer; a turn that is not running holds no running step.
   */
  state: TurnState;
  startedAt?: string;
  endedAt?: string;
  /** The turn's agent first, then the orchestrator's gate nodes (checks, CI reports, reworks). */
  roots: StepNode[];
  summary: StepSummary;
};

// ---- labels ----------------------------------------------------------------------------------

/** The longest label `current` quotes. */
const CURRENT_MAX = 80;

const firstLine = (text: string): string => text.split("\n", 1)[0] ?? "";

/** A command's label is the command itself: the first line, for a phrase. */
const commandLine = (label: string): string => truncate(firstLine(label).trim(), CURRENT_MAX).text;

/**
 * What a node is called in a sentence: "Running npm test" for a command that runs, the label for
 * the rest (a command that is done is its command line).
 */
export function phraseOf(node: Pick<StepNode, "kind" | "label" | "state">): string {
  if (node.kind === "command") {
    return node.state === "running"
      ? `Running ${commandLine(node.label)}`
      : commandLine(node.label);
  }
  const label = node.kind === "tool" ? toolName(node.label).title : node.label;
  return truncate(firstLine(label).trim(), CURRENT_MAX).text;
}

/** The plain-text label of a `working` status. */
function statusLabel(status: StatusContent): string {
  switch (status.status) {
    case "working":
      return status.detail?.trim() || "Started working";
    case "submitted":
      return "Queued";
    case "auth_required":
      return status.detail ? `Needs you to sign in: ${status.detail}` : "Needs you to sign in";
    case "canceled":
      return "Stopped";
    default:
      return status.detail?.trim() || status.status;
  }
}

/** The plain-text label of an artifact, as its step says it. */
function artifactLabel(artifact: ArtifactContent): string {
  switch (artifact.kind) {
    case "branch":
      return `Pushed ${artifact.branch ?? artifact.name}`;
    case "checks":
      return artifact.passed === true ? "Checks passed" : "Checks failed";
    default: {
      const pr = pullRequestOf(artifact);
      if (pr) {
        return pr.number !== undefined
          ? `Opened pull request #${pr.number}`
          : "Opened a pull request";
      }
      return `Shared ${artifact.name}`;
    }
  }
}

// ---- building a turn -------------------------------------------------------------------------

/** The coder's tool that runs the repository's checks (its step is `failed` when they are red). */
const RUN_CHECKS = "run_checks";

const STEP_PART = activityPartName(ACTIVITY.step);
const STATUS_PART = activityPartName(ACTIVITY.status);
const ARTIFACT_PART = activityPartName(ACTIVITY.artifact);
const CHECK_PART = activityPartName(ACTIVITY.check);
const CI_PART = activityPartName(ACTIVITY.ci);
const REWORK_PART = activityPartName(ACTIVITY.rework);
const ACTION_PART = activityPartName(ACTIVITY.action);

const isLive = (state: TurnState): boolean => state === "running" || state === "verifying";

/** The state of the turn's own root: the turn's outcome, `verifying` being a kind of running. */
const rootState = (state: TurnState): StepState => (state === "verifying" ? "running" : state);

/** The agent's last word on its status was a question for the person (`input_required`, `auth_required`). */
function endedOnQuestion(message: StepMessage): boolean {
  for (let i = message.content.length - 1; i >= 0; i--) {
    const part = message.content[i];
    if (part?.type !== "data" || part.name !== STATUS_PART) continue;
    const status = parseStatus(part.data)?.status;
    return status === "input_required" || status === "auth_required";
  }
  return false;
}

function turnStateOf(message: StepMessage, view: TurnView, isLast: boolean): TurnState {
  const status = message.status;
  if (status?.type === "running") {
    return isLast && view.state === "verifying" ? "verifying" : "running";
  }
  if (status?.type === "requires-action") return "waiting";
  // a turn that ended on a question stays paused in the history, after the answer started the next
  if (endedOnQuestion(message)) return "waiting";
  if (status?.type === "incomplete") {
    return status.reason === "cancelled"
      ? "canceled"
      : status.reason === "error"
        ? "failed"
        : "completed";
  }
  return isLast && view.waiting === true ? "waiting" : "completed";
}

function newestAndOldest(times: string[]): { first?: string; last?: string } {
  let first: string | undefined;
  let last: string | undefined;
  let lo = Number.POSITIVE_INFINITY;
  let hi = Number.NEGATIVE_INFINITY;
  for (const t of times) {
    const ms = Date.parse(t);
    if (Number.isNaN(ms)) continue;
    if (ms < lo) {
      lo = ms;
      first = t;
    }
    if (ms >= hi) {
      hi = ms;
      last = t;
    }
  }
  return { ...(first ? { first } : {}), ...(last ? { last } : {}) };
}

/** A step report as a node (the report of a step the tree has not seen yet). */
export function stepNode(content: StepContent, part?: StepNode["part"]): StepNode {
  return {
    id: content.id,
    kind: content.kind,
    label: content.label,
    state: content.state,
    ...(content.icon ? { icon: content.icon } : {}),
    ...(content.detail ? { detail: content.detail } : {}),
    ...(content.input ? { input: content.input } : {}),
    ...(content.output ? { output: content.output } : {}),
    ...(content.ioDropped ? { ioDropped: true } : {}),
    ...((content.startedAt ?? content.at) ? { startedAt: content.startedAt ?? content.at } : {}),
    ...(content.at ? { at: content.at } : {}),
    ...(part ? { part } : {}),
    children: [],
  } as StepNode;
}

function* walk(nodes: readonly StepNode[]): Generator<StepNode> {
  for (const node of nodes) {
    yield node;
    yield* walk(node.children);
  }
}

type Counts = { total: number; failed: number; running: number };
const counted = new WeakMap<StepNode, Counts>();

/** What is under a node: how many steps, how many failed, how many are running. Memoised. */
export function countUnder(node: StepNode): Counts {
  const known = counted.get(node);
  if (known) return known;
  const counts: Counts = { total: 0, failed: 0, running: 0 };
  for (const child of node.children) {
    counts.total += 1;
    if (child.state === "failed" && !child.echo) counts.failed += 1;
    if (child.state === "running") counts.running += 1;
    const under = countUnder(child);
    counts.total += under.total;
    counts.failed += under.failed;
    counts.running += under.running;
  }
  counted.set(node, counts);
  return counts;
}

/** The deepest running node (the later of equals), else the last node in reading order. */
function currentOf(roots: readonly StepNode[]): StepNode | undefined {
  let best: { node: StepNode; depth: number } | undefined;
  let last: StepNode | undefined;
  const visit = (nodes: readonly StepNode[], depth: number) => {
    for (const node of nodes) {
      if (node.kind !== "agent") {
        last = node;
        if (node.state === "running" && (!best || depth >= best.depth)) best = { node, depth };
      }
      visit(node.children, depth + 1);
    }
  };
  visit(roots, 0);
  return (best?.node ?? last) as StepNode | undefined;
}

function summaryOf(roots: readonly StepNode[], durationMs: number | undefined): StepSummary {
  const total = { total: 0, failed: 0, running: 0 };
  for (const node of walk(roots)) {
    if (node.kind === "agent") continue;
    total.total += 1;
    if (node.state === "failed" && !node.echo) total.failed += 1;
    if (node.state === "running") total.running += 1;
  }
  const current = currentOf(roots);
  return {
    ...total,
    ...(durationMs !== undefined ? { durationMs } : {}),
    ...(current ? { current: phraseOf(current) } : {}),
  };
}

/**
 * A turn that is not running holds no running step: its agent is waiting for the person (the
 * step is waiting too), or it ended and the step with it.
 */
function settle(nodes: StepNode[], turn: TurnState) {
  if (isLive(turn)) return;
  for (const node of walk(nodes)) {
    if (node.state === "running") node.state = turn === "waiting" ? "waiting" : "canceled";
  }
}

/**
 * `run_checks` reports the same failure twice: its own step ends `failed`, and the `checks`
 * artifact it made (a red `passed: false`) is a node of its own, right beside it. The artifact stays
 * a row (it holds the findings) but is marked an echo, so the failure is counted once. A red
 * artifact of a `run_checks` that did not fail (or that has no step before it) is the only
 * failure there is, and counts.
 */
function markEchoes(nodes: StepNode[]) {
  let lastRunChecks: StepNode | undefined;
  for (const node of nodes) {
    if (node.kind === "tool" && node.label === RUN_CHECKS) {
      lastRunChecks = node;
    } else if (
      node.kind === "artifact" &&
      node.content.kind === "checks" &&
      node.state === "failed"
    ) {
      if (lastRunChecks?.state === "failed") node.echo = true;
      lastRunChecks = undefined;
    }
  }
}

function build(message: StepMessage, number: number, turn: TurnState, view: TurnView): TurnSteps {
  const parts = message.content;
  const live = isLive(turn);
  const marker = parts.find((p) => p.type === "data" && p.name === ACTOR_PART);
  const actor = marker ? parseActor(marker.data) : undefined;

  const agent: StepNode = {
    id: `turn-${message.id}`,
    kind: "agent",
    label: actor?.name ?? view.agentId ?? "Agent",
    state: rootState(turn),
    children: [],
  };
  const gate: StepNode[] = [];
  const steps = new Map<string, StepNode>();
  const times: string[] = [];

  // the last part that draws a step: while the turn runs, a status or an artifact there is the
  // step the agent is on (a check that waits spins on its own)
  let lastDrawn = -1;
  parts.forEach((p, i) => {
    if (drawsStep(p)) lastDrawn = i;
  });

  parts.forEach((part, index) => {
    if (part.type === "data" && part.name?.startsWith("agui-activity/vymalo.")) {
      const at = (part.data as { at?: unknown } | undefined)?.at;
      if (typeof at === "string") times.push(at);
    }
    if (!drawsStep(part)) return;
    const ref = { messageId: message.id, index };
    const id = `part-${message.id}-${index}`;
    const running = live && index === lastDrawn;
    switch (part.name) {
      case STEP_PART: {
        const content = parseStep(part.data);
        if (!content) return;
        const known = steps.get(content.id);
        if (known) {
          // the same step says itself again (an update, its end, a retry): in place
          known.label = content.label;
          known.state = content.state;
          if (content.icon) known.icon = content.icon;
          if (content.detail !== undefined) known.detail = content.detail;
          else delete known.detail;
          // the input comes with the start and every snapshot says it again; the output only with the end
          if (content.input) known.input = content.input;
          if (content.output) known.output = content.output;
          if (content.ioDropped) known.ioDropped = true;
          if (content.at) known.at = content.at;
          known.part = ref;
          return;
        }
        const node = stepNode(content, ref);
        steps.set(content.id, node);
        const parentId = content.path.at(-1);
        const parent =
          parentId !== undefined && parentId !== content.id ? steps.get(parentId) : undefined;
        (parent ?? agent).children.push(node);
        return;
      }
      case STATUS_PART: {
        const status = parseStatus(part.data);
        if (!status) return;
        const detail = status.detail?.trim();
        const command = status.status === "working" && detail ? commandOf(detail) : undefined;
        const state: StepState =
          status.status === "canceled"
            ? "canceled"
            : status.status === "auth_required"
              ? "waiting"
              : running
                ? "running"
                : "completed";
        const at = status.at ? { startedAt: status.at, at: status.at } : {};
        if (command) {
          agent.children.push({
            id,
            kind: "command",
            label: command,
            state,
            icon: "execute",
            ...at,
            part: ref,
            children: [],
          });
          return;
        }
        agent.children.push({
          id,
          kind: "status",
          label: statusLabel(status),
          state,
          content: status,
          ...at,
          part: ref,
          children: [],
        });
        return;
      }
      case ARTIFACT_PART: {
        const artifact = parseArtifact(part.data);
        if (!artifact) return;
        const failedChecks = artifact.kind === "checks" && artifact.passed !== true;
        agent.children.push({
          id,
          kind: "artifact",
          label: artifactLabel(artifact),
          state: failedChecks ? "failed" : running ? "running" : "completed",
          content: artifact,
          ...(artifact.at ? { startedAt: artifact.at, at: artifact.at } : {}),
          part: ref,
          children: [],
        });
        return;
      }
      case ACTION_PART: {
        const action = parseAction(part.data);
        if (!action) return;
        agent.children.push({
          id,
          kind: "action",
          label: `Chose ${action.name}`,
          state: "completed",
          content: action,
          ...(action.at ? { startedAt: action.at, at: action.at } : {}),
          part: ref,
          children: [],
        });
        return;
      }
      case CHECK_PART: {
        const check = parseCheck(part.data);
        if (!check) return;
        // a pending check spins while the run is open; after it a check nobody answered decided
        // nothing, and neither did a stale answer
        const state: StepState = check.stale
          ? "canceled"
          : check.status === "pending"
            ? live
              ? "running"
              : "canceled"
            : check.status === "passed"
              ? "completed"
              : "failed";
        gate.push({
          id,
          kind: "check",
          label: checkLabel(check),
          state,
          content: check,
          ...(check.at ? { startedAt: check.at, at: check.at } : {}),
          part: ref,
          children: [],
        });
        return;
      }
      case CI_PART: {
        const ci = parseCi(part.data);
        if (!ci) return;
        gate.push({
          id,
          kind: "ci",
          label: `CI ${truncate(ci.name, 80).text}: ${conclusionLabel(ci.conclusion)}`,
          state: ci.passed ? "completed" : "failed",
          content: ci,
          ...(ci.at ? { startedAt: ci.at, at: ci.at } : {}),
          part: ref,
          children: [],
        });
        return;
      }
      case REWORK_PART: {
        const rework = parseRework(part.data);
        if (!rework) return;
        gate.push({
          id,
          kind: "rework",
          label: reworkLabel(rework),
          state: "completed",
          content: rework,
          ...(rework.at ? { startedAt: rework.at, at: rework.at } : {}),
          part: ref,
          children: [],
        });
        return;
      }
      default:
    }
  });

  markEchoes(agent.children);
  const roots = [agent, ...gate];
  settle(roots, turn);

  const { first, last } = newestAndOldest(times);
  const durationMs = first && last && !live ? Date.parse(last) - Date.parse(first) : undefined;
  return {
    turnId: message.id,
    number,
    agent: { name: agent.label, ...(actor?.revision ? { revision: actor.revision } : {}) },
    state: turn,
    ...(first ? { startedAt: first } : {}),
    ...(last && !live ? { endedAt: last } : {}),
    roots,
    summary: summaryOf(roots, durationMs),
  };
}

// ---- the thread's turns ----------------------------------------------------------------------

/**
 * One turn is rebuilt only when its message changed: the runtime keeps a message that did not
 * change as the same object, so a long thread costs the new frame and not the whole thread.
 */
const cache = new WeakMap<StepMessage, Map<string, TurnSteps>>();
const draws = new WeakMap<StepMessage, boolean>();

/** Whether a message is an agent turn: an assistant message that draws something (or is running). */
export function isAgentTurn(message: StepMessage): boolean {
  if (message.role !== "assistant") return false;
  let drawn = draws.get(message);
  if (drawn === undefined) {
    drawn = message.content.some(drawsPart);
    draws.set(message, drawn);
  }
  return drawn || message.status?.type === "running";
}

/** The agent turns of a thread, in chat order, each with its tree and its summary. */
export function buildTurnSteps(messages: readonly StepMessage[], view: TurnView): TurnSteps[] {
  const turns: TurnSteps[] = [];
  messages.forEach((message, i) => {
    if (!isAgentTurn(message)) return;
    const isLast = i === messages.length - 1;
    const number = turns.length + 1;
    const turn = turnStateOf(message, view, isLast);
    const key = `${number}|${turn}|${view.agentId ?? ""}`;
    let byKey = cache.get(message);
    if (!byKey) {
      byKey = new Map();
      cache.set(message, byKey);
    }
    let built = byKey.get(key);
    if (!built) {
      built = build(message, number, turn, view);
      byKey.set(key, built);
    }
    turns.push(built);
  });
  return turns;
}

// ---- reading a tree --------------------------------------------------------------------------

/** The latest `shown` children of a node, and every failed one: what an opened level lists. */
export function visibleChildren(
  node: Pick<StepNode, "children">,
  shown: number,
): { nodes: StepNode[]; hidden: number } {
  const all = node.children;
  const from = Math.max(0, all.length - Math.max(0, shown));
  const nodes = all.filter((child, i) => i >= from || child.state === "failed");
  return { nodes, hidden: all.length - nodes.length };
}

/** `42s`, `2m 10s`, `1h 5m`: how long something took; nothing under a second. */
export function formatDuration(ms: number): string | undefined {
  if (!Number.isFinite(ms) || ms < 1000) return undefined;
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 === 0 ? `${m}m` : `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return m % 60 === 0 ? `${h}h` : `${h}h ${m % 60}m`;
}

/** How long a finished node ran, from its own times. */
export function nodeDuration(
  node: Pick<StepNode, "state" | "startedAt" | "at">,
): string | undefined {
  if (node.state === "running" || node.state === "waiting") return undefined;
  if (!node.startedAt || !node.at) return undefined;
  return formatDuration(Date.parse(node.at) - Date.parse(node.startedAt));
}

export const plural = (n: number, one: string, many = `${one}s`): string =>
  `${n} ${n === 1 ? one : many}`;

export type SummaryIcon = "spinner" | "check" | "cross" | "pause" | "verifying" | "stopped";

/**
 * The one line a turn keeps in the chat: while it runs the step it is on, paused on a question,
 * verifying, done with its steps and how long they took, failed, stopped. A turn of words only
 * that is not running has none. `failed` counts the failed steps, which the line says even when
 * the turn succeeded.
 */
export function summaryLine(
  turn: Pick<TurnSteps, "state" | "summary">,
): { icon: SummaryIcon; text: string; failed: number } | null {
  const { total, failed, current, durationMs } = turn.summary;
  const steps = plural(total, "step");
  switch (turn.state) {
    case "running": {
      const on = current ?? "Working";
      return { icon: "spinner", text: total > 0 ? `${on} · ${steps}` : on, failed };
    }
    case "verifying":
      return { icon: "verifying", text: "Verifying", failed };
    case "waiting":
      return total > 0 ? { icon: "pause", text: `Paused · ${steps}`, failed } : null;
    case "failed":
      return total > 0 ? { icon: "cross", text: `Failed · ${steps}`, failed } : null;
    case "canceled":
      return total > 0 ? { icon: "stopped", text: `Stopped · ${steps}`, failed } : null;
    case "completed": {
      if (total === 0) return null;
      const took = durationMs !== undefined ? formatDuration(durationMs) : undefined;
      return { icon: "check", text: took ? `${steps} · ${took}` : steps, failed };
    }
    default:
      return null;
  }
}

/** The nodes from a turn's roots down to the node with this id, outermost first; empty when it is not there. */
export function pathTo(turn: Pick<TurnSteps, "roots">, id: string): StepNode[] {
  const visit = (nodes: readonly StepNode[], trail: StepNode[]): StepNode[] | undefined => {
    for (const node of nodes) {
      if (node.id === id) return [...trail, node];
      const found = visit(node.children, [...trail, node]);
      if (found) return found;
    }
    return undefined;
  };
  return visit(turn.roots, []) ?? [];
}

const STEP_KINDS_OF_AGENT: ReadonlySet<StepKind> = new Set([
  "subagent",
  "tool",
  "command",
  "message",
]);

/**
 * The first step of a turn that failed and is counted so (a step of the agent, else any other
 * failed node): where the failed chip of the chat's line takes the person. Undefined when none.
 */
export function firstFailed(turn: Pick<TurnSteps, "roots">): StepNode | undefined {
  let other: StepNode | undefined;
  for (const node of walk(turn.roots)) {
    if (node.state !== "failed" || node.echo) continue;
    if (STEP_KINDS_OF_AGENT.has(node.kind)) return node;
    other ??= node;
  }
  return other;
}

/** A step has something to open: what it was called with, what it returned, or a note that it was not kept. */
export const hasIo = (node: Pick<StepNode, "input" | "output" | "ioDropped">): boolean =>
  node.input !== undefined || node.output !== undefined || node.ioDropped === true;
