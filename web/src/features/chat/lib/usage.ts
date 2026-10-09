/**
 * Token usage of a thread (ADR 0056, docs/api/usage-v1.md), folded from the two `CUSTOM` events of the
 * connect stream: `vymalo.usage` (one model call) and `vymalo.usage_total` (a task's totals when it
 * ended or paused). The fold is pure and keyed by the task and the call, so a replay, a reconnect and
 * the live stream give the same state. What the ring and its details show is `summarize`'s.
 */

export const USAGE_EVENT = "vymalo.usage";
export const USAGE_TOTAL_EVENT = "vymalo.usage_total";

/** Who spent the tokens of a call: the thread's agent, one of its sub-agent steps, or an asked agent. */
export type UsageBy = { kind: "agent" | "subagent" | "ask"; name: string };

export type TokenCounts = {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  reasoningTokens?: number;
  cachedInputTokens?: number;
  cacheWriteInputTokens?: number;
};

export type UsageCall = {
  task: string;
  call: string;
  provider?: string;
  model: string;
  counts: TokenCounts;
  /** The model's context window as the deployment configured it; absent when not configured. */
  contextWindow?: number;
  by: UsageBy;
};

export type ModelCounts = { provider?: string; model: string; counts: TokenCounts };

export type ThreadUsage = {
  /** Every call reported, once each, in the order the stream said them. */
  calls: readonly UsageCall[];
  /** The latest totals of each task, and how many calls the thread had when they came. */
  totals: Readonly<Record<string, { entries: readonly ModelCounts[]; after: number }>>;
};

export const NO_USAGE: ThreadUsage = { calls: [], totals: {} };

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const MAX_COUNT = Number.MAX_SAFE_INTEGER;
const count = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= MAX_COUNT ? v : undefined;
const label = (v: unknown): string | undefined =>
  typeof v === "string" && v.length > 0 ? v : undefined;

const PARTS = ["reasoningTokens", "cachedInputTokens", "cacheWriteInputTokens"] as const;

function countsOf(v: Record<string, unknown>): TokenCounts | undefined {
  const inputTokens = count(v.inputTokens);
  const outputTokens = count(v.outputTokens);
  const totalTokens = count(v.totalTokens);
  if (inputTokens === undefined || outputTokens === undefined || totalTokens === undefined) {
    return undefined;
  }
  const out: TokenCounts = { inputTokens, outputTokens, totalTokens };
  for (const p of PARTS) {
    const n = count(v[p]);
    if (n !== undefined) out[p] = n;
  }
  return out;
}

function byOf(v: unknown, agent: string): UsageBy {
  if (isRecord(v) && (v.kind === "agent" || v.kind === "subagent" || v.kind === "ask")) {
    return { kind: v.kind, name: label(v.name) ?? agent };
  }
  return { kind: "agent", name: agent };
}

function modelOf(v: Record<string, unknown>): ModelCounts | undefined {
  const model = label(v.model);
  const counts = countsOf(v);
  if (!model || !counts) return undefined;
  const provider = label(v.provider);
  return { ...(provider ? { provider } : {}), model, counts };
}

/** A `vymalo.usage` value as a call; undefined for one that does not say what the contract says. */
export function parseCall(value: unknown): UsageCall | undefined {
  if (!isRecord(value)) return undefined;
  const task = label(value.task);
  const call = label(value.call);
  const agent = label(value.agent) ?? "";
  const m = modelOf(value);
  if (!task || !call || !m) return undefined;
  const window = count(value.contextWindow);
  return {
    task,
    call,
    ...(m.provider ? { provider: m.provider } : {}),
    model: m.model,
    counts: m.counts,
    ...(window ? { contextWindow: window } : {}),
    by: byOf(value.by, agent),
  };
}

/** A `vymalo.usage_total` value: the task and its totals; undefined for one that does not parse. */
export function parseTotal(value: unknown): { task: string; entries: ModelCounts[] } | undefined {
  if (!isRecord(value) || !Array.isArray(value.totals)) return undefined;
  const task = label(value.task);
  if (!task) return undefined;
  const entries = value.totals.flatMap((t) => {
    const m = isRecord(t) ? modelOf(t) : undefined;
    return m ? [m] : [];
  });
  return { task, entries };
}

/** Folds one `CUSTOM` event; any other event, or a value that does not parse, leaves the state as it is. */
export function foldUsage(state: ThreadUsage, name: unknown, value: unknown): ThreadUsage {
  if (name === USAGE_EVENT) {
    const call = parseCall(value);
    if (!call) return state;
    // a call is said once per task and id: a replay of it changes nothing
    if (state.calls.some((c) => c.task === call.task && c.call === call.call)) return state;
    return { ...state, calls: [...state.calls, call] };
  }
  if (name === USAGE_TOTAL_EVENT) {
    const total = parseTotal(value);
    if (!total) return state;
    const known = state.totals[total.task];
    if (known && sameEntries(known.entries, total.entries)) return state;
    return {
      ...state,
      totals: {
        ...state.totals,
        [total.task]: { entries: total.entries, after: state.calls.length },
      },
    };
  }
  return state;
}

const sameEntries = (a: readonly ModelCounts[], b: readonly ModelCounts[]) =>
  JSON.stringify(a) === JSON.stringify(b);

// ---- what the screen shows -----------------------------------------------------------------

/** How full the context is: neutral below 80 %, amber from 80 %, red from 95 % (ADR 0056). */
export type FillLevel = "normal" | "warn" | "danger";

export const WARN_AT = 0.8;
export const DANGER_AT = 0.95;

export type UsageGroup = UsageBy & { calls: number; counts: TokenCounts };

export type UsageSummary = {
  /** The latest call of the thread's agent (`by.kind` `agent`): what the ring fills with. */
  latest: UsageCall | undefined;
  /** `inputTokens / contextWindow` of `latest`, at most 1; undefined without a window. */
  fill: { ratio: number; level: FillLevel } | undefined;
  /** The thread's totals per provider and model: each task's latest totals plus the calls after them, else its calls. */
  models: ModelCounts[];
  /** The calls of the agent, of each sub-agent and of each asked agent, apart, in the order they first spent. */
  groups: UsageGroup[];
};

const ZERO: TokenCounts = { inputTokens: 0, outputTokens: 0, totalTokens: 0 };

function plus(a: TokenCounts, b: TokenCounts): TokenCounts {
  const out: TokenCounts = {
    inputTokens: a.inputTokens + b.inputTokens,
    outputTokens: a.outputTokens + b.outputTokens,
    totalTokens: a.totalTokens + b.totalTokens,
  };
  for (const p of PARTS) {
    if (a[p] !== undefined || b[p] !== undefined) out[p] = (a[p] ?? 0) + (b[p] ?? 0);
  }
  return out;
}

export const levelOf = (ratio: number): FillLevel =>
  ratio >= DANGER_AT ? "danger" : ratio >= WARN_AT ? "warn" : "normal";

export function summarize(state: ThreadUsage): UsageSummary {
  const latest = state.calls.findLast((c) => c.by.kind === "agent");
  const fill =
    latest?.contextWindow !== undefined && latest.contextWindow > 0
      ? (() => {
          const ratio = Math.min(1, latest.counts.inputTokens / latest.contextWindow);
          return { ratio, level: levelOf(ratio) };
        })()
      : undefined;

  const models = new Map<string, ModelCounts>();
  const add = (m: { provider?: string; model: string }, counts: TokenCounts) => {
    const key = `${m.provider ?? ""}\u0000${m.model}`;
    const had = models.get(key);
    models.set(key, {
      ...(m.provider ? { provider: m.provider } : {}),
      model: m.model,
      counts: plus(had?.counts ?? ZERO, counts),
    });
  };
  for (const [, total] of Object.entries(state.totals)) {
    for (const e of total.entries) add(e, e.counts);
  }
  state.calls.forEach((c, i) => {
    const total = state.totals[c.task];
    // a task's totals cover its calls before them
    if (total && i < total.after) return;
    add(c, c.counts);
  });

  const groups = new Map<string, UsageGroup>();
  for (const c of state.calls) {
    const key = `${c.by.kind}\u0000${c.by.name}`;
    const had = groups.get(key);
    groups.set(key, {
      ...c.by,
      calls: (had?.calls ?? 0) + 1,
      counts: plus(had?.counts ?? ZERO, c.counts),
    });
  }
  const rank = { agent: 0, subagent: 1, ask: 2 } as const;
  return {
    latest,
    fill,
    models: [...models.values()].sort(
      (a, b) => b.counts.totalTokens - a.counts.totalTokens || a.model.localeCompare(b.model),
    ),
    groups: [...groups.values()].sort((a, b) => rank[a.kind] - rank[b.kind]),
  };
}

/** Whether there is anything to show: a thread with no usage shows no ring. */
export const hasUsage = (state: ThreadUsage): boolean =>
  state.calls.length > 0 || Object.keys(state.totals).length > 0;
