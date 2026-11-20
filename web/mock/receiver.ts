/**
 * The receiving side of a form (docs/api/ui-catalog-v1.md, Version 7, "Receiving an answer"): what
 * the agent that asked does with the answer before it acts on it. The orchestrator does not read an
 * answer, so in this mock it is the scripted agent that validates, as an adam agent does.
 *
 * Written from the contract page alone, not from the web's own check (`src/features/chat/lib/a2ui/
 * choices-kinds.ts`), so that the two implementations meeting on the page's conformance vectors
 * (`receiver.test.ts`) prove the page says enough for a second client. An answer that fails is not
 * acted on, never repaired: no clamping, no rounding, no sorting.
 */

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);

/** One refusal: the question (or `*` for the whole answer), the rule of the contract's table, and words. */
export type Refusal = { id: string; rule: string; reason: string };

const DECIMAL = /^(-?)(0|[1-9][0-9]{0,14})(?:\.([0-9]{1,6}))?$/;
const MICRO = 1_000_000n;

/** A decimal string as millionths, or undefined (the contract's grammar; zero has no sign). */
function decimal(text: unknown): bigint | undefined {
  if (typeof text !== "string") return undefined;
  const m = DECIMAL.exec(text);
  if (!m) return undefined;
  const value = BigInt(m[2] ?? "0") * MICRO + BigInt((m[3] ?? "").padEnd(6, "0"));
  if (value === 0n && m[1] === "-") return undefined;
  return m[1] === "-" ? -value : value;
}

function realDay(text: string): boolean {
  const m = /^([0-9]{4})-([0-9]{2})-([0-9]{2})$/.exec(text);
  if (!m) return false;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  if (y < 1 || mo < 1 || mo > 12 || d < 1) return false;
  const leap = (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0;
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return d <= (days[mo - 1] ?? 0);
}

const unit = (q: Rec) => (typeof q.unit === "string" ? ` ${q.unit}` : "");

/** The first rule that fails for the `values` of one question of the five kinds, or undefined. */
export function refusalOf(q: Rec, values: unknown): Omit<Refusal, "id"> | undefined {
  if (!Array.isArray(values)) return { rule: "unknown", reason: "values is not a list" };
  const v = values as unknown[];
  const required = q.required !== false;
  if (v.length === 0) {
    return required ? { rule: "required", reason: "it needs an answer" } : undefined;
  }
  if (q.type === "rank") {
    const options = (Array.isArray(q.options) ? q.options : []).map((o) => (o as Rec).value);
    const stray = v.find((x) => !options.includes(x));
    if (stray !== undefined) return { rule: "not_offered", reason: `${stray} is not an option` };
    if (new Set(v).size !== v.length || v.length !== options.length) {
      return { rule: "not_a_permutation", reason: "every option is needed once" };
    }
    return undefined;
  }
  if (v.length > 1) return { rule: "too_many", reason: "one answer is expected" };
  const value = v[0];
  if (typeof value !== "string") {
    // not a string: refused as the kind's own rule refuses a wrong one
    return q.type === "confirm"
      ? { rule: "not_offered", reason: "it is not yes or no" }
      : { rule: "grammar", reason: "it is not text" };
  }
  switch (q.type) {
    case "confirm":
      return value === "yes" || value === "no"
        ? undefined
        : { rule: "not_offered", reason: `${JSON.stringify(value)} is not yes or no` };
    case "number": {
      const n = decimal(value);
      if (n === undefined)
        return { rule: "grammar", reason: `${JSON.stringify(value)} is not a decimal` };
      const min = decimal(q.minValue);
      const max = decimal(q.maxValue);
      const step = decimal(q.step);
      if (min !== undefined && n < min) {
        return {
          rule: "below_min",
          reason: `${value} is below the least, ${String(q.minValue)}${unit(q)}`,
        };
      }
      if (max !== undefined && n > max) {
        return {
          rule: "above_max",
          reason: `${value} is above the most, ${String(q.maxValue)}${unit(q)}`,
        };
      }
      if (step !== undefined && step > 0n && (n - (min ?? 0n)) % step !== 0n) {
        return { rule: "off_step", reason: `${value} is not a multiple of ${String(q.step)}` };
      }
      return undefined;
    }
    case "scale": {
      if (!/^(0|[1-9][0-9]?)$/.test(value)) {
        return { rule: "grammar", reason: `${JSON.stringify(value)} is not a whole number` };
      }
      const from = typeof q.from === "number" ? q.from : 1;
      const to = typeof q.to === "number" ? q.to : from;
      const n = Number(value);
      if (n < from) return { rule: "below_min", reason: `${value} is below ${from}` };
      if (n > to) return { rule: "above_max", reason: `${value} is above ${to}` };
      return undefined;
    }
    case "date": {
      if (!realDay(value))
        return { rule: "grammar", reason: `${JSON.stringify(value)} is not a day` };
      if (typeof q.minDate === "string" && value < q.minDate) {
        return { rule: "below_min", reason: `${value} is before ${q.minDate}` };
      }
      if (typeof q.maxDate === "string" && value > q.maxDate) {
        return { rule: "above_max", reason: `${value} is after ${q.maxDate}` };
      }
      return undefined;
    }
    default:
      return undefined;
  }
}

/**
 * Every refusal of an action's `context` against the questions the agent asked, in question order:
 * the answers must be the questions, once each, in order (`unknown`), carry nothing but `id` and
 * `values` (`unexpected`), and each pass its question's rules.
 */
export function receive(
  questions: readonly Rec[],
  context: unknown,
  form: { freeInput?: boolean } = {},
): Refusal[] {
  const answers = isRecord(context) ? context.answers : undefined;
  if (!Array.isArray(answers)) {
    return [{ id: "*", rule: "unknown", reason: "the answer has no answers" }];
  }
  const members = Object.keys(context as Rec).filter((k) => k !== "answers" && k !== "note");
  const note = "note" in (context as Rec) && !form.freeInput;
  if (members.length > 0 || note) {
    return [
      { id: "*", rule: "unexpected", reason: "the context carries more than the form allows" },
    ];
  }
  const ids = answers.map((a) => (isRecord(a) ? a.id : undefined));
  if (ids.length !== questions.length || ids.some((id, i) => id !== (questions[i] as Rec).id)) {
    return [{ id: "*", rule: "unknown", reason: "the answers are not the questions, in order" }];
  }
  const refusals: Refusal[] = [];
  for (const [i, question] of questions.entries()) {
    const answer = answers[i] as Rec;
    const id = String(question.id);
    if (Object.keys(answer).some((k) => k !== "id" && k !== "values")) {
      refusals.push({ id, rule: "unexpected", reason: "it carries more than id and values" });
      continue;
    }
    const refusal = refusalOf(question, answer.values);
    if (refusal) refusals.push({ id, ...refusal });
  }
  return refusals;
}

/** What the agent says to the person: the question and the rule, in words, as the contract suggests. */
export const refusalText = (r: Refusal): string =>
  `The answer to "${r.id}" was not used (${r.rule}): ${r.reason}.`;
