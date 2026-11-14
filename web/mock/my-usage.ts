/**
 * The mock's `GET /api/me/usage` (`getMyUsage`, ADR 0065): the answers a session holds and how a query is read.
 * (`usage.ts` is another thing: the run accounting of `RUN_FINISHED.usage`.)
 *
 * A test sets the answers of its own session with `POST /__mock/my-usage`: a JSON object of month (`YYYY-MM`) to
 * the `MyUsage` the page is given for it, and `"*"` for the answer of every month it names none for (its `month` is
 * the month asked, so a test need not know the current month). A month nothing is set for is an empty, complete
 * answer, as the orchestrator's is for a month its ledger holds nothing for. The mock prices nothing: a test writes
 * the amounts it wants drawn.
 */
import type { components } from "../src/lib/api/schema";

export type MyUsage = components["schemas"]["MyUsage"];

const MONTH = /^[0-9]{4}-(0[1-9]|1[0-2])$/;

/** The empty month of a deployment that prices in USD: complete, at zero. */
export function emptyUsage(month: string): MyUsage {
  return {
    month,
    currency: "USD",
    prices: "sha256:0123456789abcdef",
    tasks: 0,
    models: [],
    sandbox: [],
    unmeasured: [],
    total: {
      amount: "0.000000",
      currency: "USD",
      complete: true,
      unpriced: { models: [], sandbox: 0, unmeasured: 0 },
    },
  };
}

const utcMonth = (now: Date) =>
  `${String(now.getUTCFullYear()).padStart(4, "0")}-${String(now.getUTCMonth() + 1).padStart(2, "0")}`;

/** The month a request asks for, or the 400's words: one `month` of the right shape and no other key. */
export function readQuery(
  params: URLSearchParams,
  now: Date = new Date(),
): { month: string } | { error: string } {
  for (const key of new Set(params.keys())) {
    if (key !== "month") return { error: `\`${key}\` is not a parameter of this route` };
  }
  const all = params.getAll("month");
  if (all.length > 1) return { error: "`month` is given more than once" };
  const month = all[0];
  if (month === undefined) return { month: utcMonth(now) };
  return MONTH.test(month) ? { month } : { error: "`month` is `YYYY-MM`" };
}

/** What a session holds, as `POST /__mock/my-usage` sets it: the body, or `null` when it is not that. */
export function parseAnswers(body: unknown): Record<string, MyUsage> | null {
  if (typeof body !== "object" || body === null || Array.isArray(body)) return null;
  const answers = body as Record<string, MyUsage>;
  for (const key of Object.keys(answers)) if (key !== "*" && !MONTH.test(key)) return null;
  return answers;
}

/** The answer for `month`: the session's own for it, else its `"*"`, else an empty month. */
export function answerFor(answers: Record<string, MyUsage>, month: string): MyUsage {
  const own = answers[month] ?? answers["*"];
  return own ? { ...own, month } : emptyUsage(month);
}
