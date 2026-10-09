/**
 * The mock's copy of the run accounting of token usage (ADR 0056, `orch-agui-projection`'s
 * `usage.rs`): what `RUN_FINISHED.usage` and `RUN_ERROR.usage` say. For each task of the thread's
 * agent a run touched: what is known of it (its latest totals plus the calls after them, else the
 * sum of its calls) less what an earlier run already said, summed per provider and model. An asked
 * agent's tasks are left out: their usage is said under the ask's subagent.
 */

type Tally = {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  reasoningTokens?: number;
  cachedInputTokens?: number;
  cacheWriteInputTokens?: number;
};

const PARTS = ["reasoningTokens", "cachedInputTokens", "cacheWriteInputTokens"] as const;

/** A provider and a model, as one map key; `\u0000` sorts an entry with no provider first, as the Rust `Option` does. */
const keyOf = (provider: string | undefined, model: string) =>
  `${provider === undefined ? "\u0000" : `\u0001${provider}`}\u0002${model}`;

function unkey(key: string): { provider?: string; model: string } {
  const [p = "", model = ""] = key.split("\u0002");
  return p === "\u0000" ? { model } : { provider: p.slice(1), model };
}

const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const opt = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isFinite(v) ? v : undefined;

function tallyOf(d: Record<string, unknown>): Tally {
  const t: Tally = {
    inputTokens: num(d.inputTokens),
    outputTokens: num(d.outputTokens),
    totalTokens: num(d.totalTokens),
  };
  for (const p of PARTS) {
    const v = opt(d[p]);
    if (v !== undefined) t[p] = v;
  }
  return t;
}

function plus(a: Tally, b: Tally): Tally {
  const t: Tally = {
    inputTokens: a.inputTokens + b.inputTokens,
    outputTokens: a.outputTokens + b.outputTokens,
    totalTokens: a.totalTokens + b.totalTokens,
  };
  for (const p of PARTS) {
    if (a[p] !== undefined || b[p] !== undefined) t[p] = (a[p] ?? 0) + (b[p] ?? 0);
  }
  return t;
}

function minus(a: Tally, b: Tally): Tally {
  const t: Tally = {
    inputTokens: Math.max(0, a.inputTokens - b.inputTokens),
    outputTokens: Math.max(0, a.outputTokens - b.outputTokens),
    totalTokens: Math.max(0, a.totalTokens - b.totalTokens),
  };
  for (const p of PARTS) {
    const v = a[p];
    if (v !== undefined) t[p] = Math.max(0, v - (b[p] ?? 0));
  }
  return t;
}

const isZero = (t: Tally) =>
  t.inputTokens === 0 &&
  t.outputTokens === 0 &&
  t.totalTokens === 0 &&
  PARTS.every((p) => (t[p] ?? 0) === 0);

const ZERO: Tally = { inputTokens: 0, outputTokens: 0, totalTokens: 0 };

function addInto(map: Map<string, Tally>, key: string, t: Tally) {
  map.set(key, plus(map.get(key) ?? ZERO, t));
}

type TaskUsage = {
  ask: boolean;
  totals?: Map<string, Tally>;
  since: Map<string, Tally>;
  reported: Map<string, Tally>;
};

function known(task: TaskUsage): Map<string, Tally> {
  const out = new Map(task.totals ?? []);
  for (const [key, t] of task.since) addInto(out, key, t);
  return out;
}

/** Whether a path names an ask (`ask-<n>`): an asked agent's usage. */
export const isAskPath = (path: unknown): boolean =>
  Array.isArray(path) && path.some((id) => typeof id === "string" && /^ask-\d+$/.test(id));

export class UsageFold {
  private readonly tasks = new Map<string, TaskUsage>();
  private touched = new Map<string, Set<string>>();

  private task(id: string): TaskUsage {
    let task = this.tasks.get(id);
    if (!task) {
      task = { ask: false, since: new Map(), reported: new Map() };
      this.tasks.set(id, task);
    }
    return task;
  }

  private touch(id: string, keys: Iterable<string>) {
    let set = this.touched.get(id);
    if (!set) {
      set = new Set();
      this.touched.set(id, set);
    }
    for (const k of keys) set.add(k);
  }

  /** A `model_usage`: one call's tokens. */
  call(d: Record<string, unknown>, inRun: boolean) {
    const id = String(d.task);
    const key = keyOf(typeof d.provider === "string" ? d.provider : undefined, String(d.model));
    const task = this.task(id);
    task.ask ||= isAskPath(d.path);
    addInto(task.since, key, tallyOf(d));
    if (inRun) this.touch(id, [key]);
  }

  /** A `model_usage_total`: the task's totals, which replace what its calls added. */
  total(d: Record<string, unknown>, inRun: boolean) {
    const id = String(d.task);
    const task = this.task(id);
    task.ask ||= isAskPath(d.path);
    const totals = new Map<string, Tally>();
    for (const m of Array.isArray(d.totals) ? d.totals : []) {
      const entry = m as Record<string, unknown>;
      totals.set(
        keyOf(typeof entry.provider === "string" ? entry.provider : undefined, String(entry.model)),
        tallyOf(entry),
      );
    }
    task.totals = totals;
    task.since.clear();
    if (inRun) this.touch(id, totals.keys());
  }

  /** The run closes: its accounting, or `undefined` when it touched no task of the thread's agent. */
  closeRun(): Record<string, unknown>[] | undefined {
    const touched = this.touched;
    this.touched = new Map();
    const sum = new Map<string, Tally>();
    let said = false;
    for (const [id, heard] of touched) {
      const task = this.tasks.get(id);
      if (!task || task.ask) continue;
      const now = known(task);
      for (const [key, t] of now) {
        const delta = minus(t, task.reported.get(key) ?? ZERO);
        if (!isZero(delta) || heard.has(key)) {
          addInto(sum, key, delta);
          said = true;
        }
      }
      task.reported = now;
    }
    if (!said) return undefined;
    return [...sum.keys()].sort().map((key) => ({ ...unkey(key), ...sum.get(key) }));
  }
}
