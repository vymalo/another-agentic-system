import type { ApiMyUsage } from "@/lib/api/types";

/**
 * What the usage page does with an answer it cannot trust to be whole (ADR 0065). The contract makes `models`, `sandbox`
 * and `unmeasured` required, but a page that throws on a partial answer is a blank page, so an array that is missing
 * is an empty one and the answer is `partial`, which the page says. An answer with no `total` (or no amount in it)
 * has no number the page may show, so it is `null`: the page says it could not read the month, and never invents a figure.
 */
export type ReadAnswer = { usage: ApiMyUsage; partial: boolean };

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export function readAnswer(raw: unknown): ReadAnswer | null {
  if (!isRecord(raw) || typeof raw.month !== "string" || typeof raw.tasks !== "number") return null;
  const total = raw.total;
  if (!isRecord(total) || typeof total.amount !== "string" || typeof total.complete !== "boolean") {
    return null;
  }
  let partial = false;
  const list = <T>(v: unknown): T[] => {
    if (Array.isArray(v)) return v as T[];
    partial = true;
    return [];
  };
  const models = list<ApiMyUsage["models"][number]>(raw.models);
  const sandbox = list<ApiMyUsage["sandbox"][number]>(raw.sandbox);
  const unmeasured = list<ApiMyUsage["unmeasured"][number]>(raw.unmeasured);
  const u = isRecord(total.unpriced) ? total.unpriced : {};
  const count = (v: unknown): number => {
    if (typeof v === "number") return v;
    partial = true;
    return 0;
  };
  const unpriced = {
    models: list<ApiMyUsage["total"]["unpriced"]["models"][number]>(u.models),
    sandbox: count(u.sandbox),
    unmeasured: count(u.unmeasured),
  };
  return {
    partial,
    usage: {
      ...(raw as unknown as ApiMyUsage),
      models,
      sandbox,
      unmeasured,
      total: { ...(total as unknown as ApiMyUsage["total"]), unpriced },
    },
  };
}
