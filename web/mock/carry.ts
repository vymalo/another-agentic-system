/**
 * The mock's copy of the carry the orchestrator writes for a page of history (`orch-agui-projection`, `carry.rs`;
 * docs/api/history.md, "Carry"): what the log before the page contributes to the readouts that cover the whole thread.
 * `history.test.ts` checks it against the fixtures the real fold writes (docs/api/examples/history/*.walk.json), so this
 * cannot drift unnoticed.
 *
 * It is read off the frames, in the order the log said them; the pass records where it stood at each chain start
 * (`prefix`) and the carry of a page is the fold of the records before its first chain.
 */
import {
  type ModelCounts,
  parseCall,
  parseTotal,
  type TokenCounts,
  USAGE_EVENT,
  USAGE_TOTAL_EVENT,
  type UsageCall,
} from "../src/features/chat/lib/usage";
import type { Frame } from "./projection";

/** The most kept files a carry names: the newest ones. */
export const MAX_FILES = 500;

export type Prefix = { usage: number; files: number; turns: number };

export type Carry = {
  turns: number;
  usage?: Record<string, unknown>;
  files?: Record<string, unknown>[];
};

type Record_ = { call: UsageCall } | { total: { task: string; entries: ModelCounts[] } };

const PARTS = ["reasoningTokens", "cachedInputTokens", "cacheWriteInputTokens"] as const;

const plus = (a: TokenCounts | undefined, b: TokenCounts): TokenCounts => {
  if (!a) return { ...b };
  const out: TokenCounts = {
    inputTokens: a.inputTokens + b.inputTokens,
    outputTokens: a.outputTokens + b.outputTokens,
    totalTokens: a.totalTokens + b.totalTokens,
  };
  for (const p of PARTS) {
    if (a[p] !== undefined || b[p] !== undefined) out[p] = (a[p] ?? 0) + (b[p] ?? 0);
  }
  return out;
};

const key = (m: { provider?: string | undefined; model: string }) =>
  `${m.provider ?? ""}\u0000${m.model}`;
const sameEntries = (a: readonly ModelCounts[], b: readonly ModelCounts[]) =>
  JSON.stringify(a) === JSON.stringify(b);

/**
 * Whether an activity is something a turn of the agent draws (`drawsPart` of src/features/chat/lib/steps.ts): a status that only
 * comes with the agent's words, the job marker, a fork's divider and a tools card draw nothing of their own.
 */
function draws(activityType: unknown, content: unknown): boolean {
  switch (activityType) {
    case "vymalo.status": {
      const status = (content as { status?: unknown } | null)?.status;
      return typeof status === "string" && status !== "completed" && status !== "input_required";
    }
    case "vymalo.artifact":
    case "vymalo.step":
    case "vymalo.ask":
    case "vymalo.check":
    case "vymalo.ci":
    case "vymalo.rework":
    case "vymalo.action":
    case "vymalo.error":
    case "a2ui-surface":
      return true;
    default:
      return false;
  }
}

export class CarryLog {
  private readonly usage: Record_[] = [];
  private readonly files: Record<string, unknown>[] = [];
  private readonly hashes = new Set<string>();
  private turns = 0;
  private output = false;

  prefix(): Prefix {
    return { usage: this.usage.length, files: this.files.length, turns: this.turns };
  }

  observe(frames: readonly Frame[]) {
    for (const { event } of frames) {
      switch (event.type) {
        case "RUN_STARTED":
          this.output = false;
          break;
        case "RUN_FINISHED":
        case "RUN_ERROR":
          if (this.output) this.turns++;
          this.output = false;
          break;
        case "TEXT_MESSAGE_START":
          if (event.role !== "user") this.output = true;
          break;
        case "REASONING_START":
        case "REASONING_MESSAGE_START":
          this.output = true;
          break;
        case "ACTIVITY_SNAPSHOT":
          this.output ||= draws(event.activityType, event.content);
          if (event.activityType === "vymalo.artifact") this.keepFile(event.content);
          break;
        case "CUSTOM":
          if (event.name === USAGE_EVENT) {
            const call = parseCall(event.value);
            if (call) this.usage.push({ call });
          } else if (event.name === USAGE_TOTAL_EVENT) {
            const total = parseTotal(event.value);
            if (total) this.usage.push({ total });
          }
          break;
        default:
      }
    }
  }

  private keepFile(content: unknown) {
    if (typeof content !== "object" || content === null) return;
    const c = content as Record<string, unknown>;
    const hash = c.sha256;
    if (c.kind !== "file" || typeof hash !== "string") return;
    if (c.href === undefined || c.size === undefined || this.hashes.has(hash)) return;
    this.hashes.add(hash);
    this.files.push(c);
  }

  /** The carry for a page that begins where `prefix` was taken. */
  carry(prefix: Prefix): Carry {
    const files = this.files.slice(0, prefix.files);
    const usage = usageCarry(this.usage.slice(0, prefix.usage));
    return {
      turns: prefix.turns,
      ...(usage ? { usage } : {}),
      ...(files.length ? { files: files.slice(Math.max(0, files.length - MAX_FILES)) } : {}),
    };
  }
}

const counts = (c: TokenCounts): TokenCounts => c;

function usageCarry(records: readonly Record_[]): Record<string, unknown> | undefined {
  if (records.length === 0) return undefined;
  const seen = new Set<string>();
  const tasks = new Map<string, { totals?: ModelCounts[]; since: ModelCounts[] }>();
  const groups = new Map<
    string,
    { kind: string; name: string; calls: number; counts: TokenCounts }
  >();
  let latest: UsageCall | undefined;
  const task = (id: string) => {
    const had = tasks.get(id);
    if (had) return had;
    const made: { totals?: ModelCounts[]; since: ModelCounts[] } = { since: [] };
    tasks.set(id, made);
    return made;
  };
  for (const r of records) {
    if ("call" in r) {
      const c = r.call;
      const id = `${c.task}\u0000${c.call}`;
      if (seen.has(id)) continue;
      seen.add(id);
      task(c.task).since.push({
        ...(c.provider ? { provider: c.provider } : {}),
        model: c.model,
        counts: c.counts,
      });
      const g = `${c.by.kind}\u0000${c.by.name}`;
      const had = groups.get(g);
      groups.set(g, {
        kind: c.by.kind,
        name: c.by.name,
        calls: (had?.calls ?? 0) + 1,
        counts: plus(had?.counts, c.counts),
      });
      if (c.by.kind === "agent") latest = c;
    } else {
      const t = task(r.total.task);
      // the same totals again change nothing: the calls since are still counted
      if (t.totals && sameEntries(t.totals, r.total.entries)) continue;
      t.totals = r.total.entries;
      t.since = [];
    }
  }
  const byTask = [...tasks.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([id, t]) => {
      const models = new Map<string, { provider?: string; model: string; counts: TokenCounts }>();
      for (const e of [...(t.totals ?? []), ...t.since]) {
        const k = key(e);
        const had = models.get(k);
        models.set(k, {
          ...(e.provider ? { provider: e.provider } : {}),
          model: e.model,
          counts: plus(had?.counts, e.counts),
        });
      }
      const sorted = [...models.values()].sort(
        (a, b) =>
          (a.provider ?? "").localeCompare(b.provider ?? "") ||
          (a.model < b.model ? -1 : a.model > b.model ? 1 : 0),
      );
      return {
        task: id,
        models: sorted.map((m) => ({
          ...(m.provider ? { provider: m.provider } : {}),
          model: m.model,
          ...counts(m.counts),
        })),
      };
    });
  return {
    tasks: byTask,
    groups: [...groups.values()].map((g) => ({
      kind: g.kind,
      name: g.name,
      calls: g.calls,
      ...g.counts,
    })),
    ...(latest
      ? {
          latest: {
            task: latest.task,
            call: latest.call,
            ...(latest.provider ? { provider: latest.provider } : {}),
            model: latest.model,
            ...latest.counts,
            ...(latest.contextWindow ? { contextWindow: latest.contextWindow } : {}),
            by: { kind: latest.by.kind, name: latest.by.name },
          },
        }
      : {}),
  };
}
