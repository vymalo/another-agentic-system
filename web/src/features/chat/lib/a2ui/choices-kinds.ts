/**
 * The five kinds of question of catalog version 7 (docs/api/ui-catalog-v1.md, "Version 7"): a yes
 * or no (`confirm`), a number, a point on a scale, a day and a list to put in order (`rank`). Pure.
 *
 * What a number is: a decimal string of a stated grammar, compared as an exact decimal (a value
 * scaled by 10^6 in a `bigint`), never through floating point. What a date is: a calendar day
 * `YYYY-MM-DD` that exists, with no time and no zone. `checkValues` is the check the contract
 * gives a receiver, and the client runs the same one before it sends: the first rule that fails
 * names the answer's problem. The rule names are the contract's (`required`, `too_many`, ...).
 */
import type { ChoiceQuestion, QuestionType } from "./choices";

export const KIND_TYPES = ["confirm", "number", "scale", "date", "rank"] as const;
/** The types of version 7: none is drawn by a client that only knows version 6. */
export const isKind = (type: QuestionType): boolean => KIND_TYPES.some((t) => t === type);

/** The rules of the contract's table "Receiving an answer", in the order a receiver tries them. */
export type Rule =
  | "required"
  | "too_many"
  | "not_offered"
  | "grammar"
  | "below_min"
  | "above_max"
  | "off_step"
  | "not_a_permutation";
export type Problem = { rule: Rule; message: string };

// ---- decimals -----------------------------------------------------------------------------

const DECIMAL = /^(-?)(0|[1-9][0-9]{0,14})(?:\.([0-9]{1,6}))?$/;
const SCALE = 10n ** 6n;

/**
 * A decimal string as an integer of millionths, or undefined when it is not the grammar:
 * `-?(0|[1-9][0-9]{0,14})(\.[0-9]{1,6})?`, and no sign on a zero (`-0`, `-0.0`).
 */
export function parseDecimal(text: string): bigint | undefined {
  const m = DECIMAL.exec(text);
  if (!m) return undefined;
  const [, sign, whole, fraction = ""] = m;
  const value = BigInt(whole as string) * SCALE + BigInt(fraction.padEnd(6, "0"));
  if (value === 0n) return sign === "-" ? undefined : 0n;
  return sign === "-" ? -value : value;
}

/** -1, 0 or 1. */
export const compareDecimals = (a: bigint, b: bigint): -1 | 0 | 1 => (a < b ? -1 : a > b ? 1 : 0);

// ---- dates --------------------------------------------------------------------------------

const DATE = /^([0-9]{4})-([0-9]{2})-([0-9]{2})$/;
const leap = (y: number) => (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0;
const DAYS = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/** A day of the proleptic Gregorian calendar, written `YYYY-MM-DD`, years 0001 to 9999. */
export function isCalendarDate(text: string): boolean {
  const m = DATE.exec(text);
  if (!m) return false;
  const year = Number(m[1]);
  const month = Number(m[2]);
  const day = Number(m[3]);
  if (year < 1 || month < 1 || month > 12 || day < 1) return false;
  return day <= (DAYS[month - 1] as number) + (month === 2 && leap(year) ? 1 : 0);
}

/**
 * A day written out for a person: `15 December 2026`. It is a day and not an instant, so it is
 * built in UTC and shown in UTC: the same day for everyone, whatever the zone of the browser.
 */
export function longDate(text: string, locale?: string): string {
  if (!isCalendarDate(text)) return text;
  const at = new Date(0);
  at.setUTCFullYear(Number(text.slice(0, 4)), Number(text.slice(5, 7)) - 1, Number(text.slice(8)));
  // display follows the viewer's locale (`undefined`: the browser's); the wire form never does
  return new Intl.DateTimeFormat(locale, { timeZone: "UTC", dateStyle: "long" }).format(at);
}

// ---- scale and confirm --------------------------------------------------------------------

const WHOLE = /^(0|[1-9][0-9]?)$/;
/** Where a scale starts: 1 unless the question says 0. */
export const scaleFrom = (q: ChoiceQuestion): number => q.from ?? 1;
/** The points of a scale, from the first to the last (empty when it has no `to`). */
export const scalePoints = (q: ChoiceQuestion): number[] =>
  q.to === undefined
    ? []
    : Array.from({ length: q.to - scaleFrom(q) + 1 }, (_, i) => scaleFrom(q) + i);
export const YES = "yes";
export const NO = "no";
export const yesLabel = (q: ChoiceQuestion): string => q.yesLabel ?? "Yes";
export const noLabel = (q: ChoiceQuestion): string => q.noLabel ?? "No";

const withUnit = (q: ChoiceQuestion, value: string): string =>
  q.unit ? `${value} ${q.unit}` : value;

// ---- the check ----------------------------------------------------------------------------

const problem = (rule: Rule, message: string): Problem => ({ rule, message });

/** What a number is, said to the person who typed something else. */
const NUMBER_GRAMMAR = "Use digits and a point, like 12.5: no spaces, commas or exponent.";

function checkNumber(q: ChoiceQuestion, value: string): Problem | undefined {
  const n = parseDecimal(value);
  if (n === undefined) return problem("grammar", NUMBER_GRAMMAR);
  const min = q.minValue === undefined ? undefined : parseDecimal(q.minValue);
  const max = q.maxValue === undefined ? undefined : parseDecimal(q.maxValue);
  if (min !== undefined && n < min) {
    return problem("below_min", `The least is ${withUnit(q, q.minValue as string)}.`);
  }
  if (max !== undefined && n > max) {
    return problem("above_max", `The most is ${withUnit(q, q.maxValue as string)}.`);
  }
  const step = q.step === undefined ? undefined : parseDecimal(q.step);
  if (step !== undefined && step > 0n && (n - (min ?? 0n)) % step !== 0n) {
    const from = q.minValue ?? "0";
    return problem("off_step", `Use steps of ${withUnit(q, q.step as string)} from ${from}.`);
  }
  return undefined;
}

function checkScale(q: ChoiceQuestion, value: string): Problem | undefined {
  const from = scaleFrom(q);
  const to = q.to ?? from;
  if (!WHOLE.test(value)) {
    return problem("grammar", `Choose a whole number from ${from} to ${to}.`);
  }
  const n = Number(value);
  if (n < from) return problem("below_min", `The least is ${from}.`);
  if (n > to) return problem("above_max", `The most is ${to}.`);
  return undefined;
}

function checkDate(q: ChoiceQuestion, value: string): Problem | undefined {
  if (!isCalendarDate(value)) return problem("grammar", "Enter a real day as YYYY-MM-DD.");
  if (q.minDate !== undefined && value < q.minDate) {
    return problem("below_min", `The earliest day is ${q.minDate}.`);
  }
  if (q.maxDate !== undefined && value > q.maxDate) {
    return problem("above_max", `The latest day is ${q.maxDate}.`);
  }
  return undefined;
}

function checkRank(q: ChoiceQuestion, values: readonly unknown[]): Problem | undefined {
  const offered = new Set<unknown>(q.options.map((o) => o.value));
  const stray = values.find((v) => !offered.has(v));
  if (stray !== undefined)
    return problem("not_offered", `${JSON.stringify(stray)} is not one of the options.`);
  if (new Set(values).size !== values.length || values.length !== q.options.length) {
    return problem("not_a_permutation", "Put every option in the list, once.");
  }
  return undefined;
}

/**
 * What a receiver, and the client before it, checks of the `values` of one question of one of the
 * five kinds: undefined when the answer is acceptable, else the first rule that fails. An empty
 * `values` is accepted only when the question is not required.
 */
export function checkValues(q: ChoiceQuestion, values: readonly unknown[]): Problem | undefined {
  if (values.length === 0) {
    return q.required ? problem("required", "Answer this question.") : undefined;
  }
  if (q.type === "rank") return checkRank(q, values);
  if (values.length > 1) return problem("too_many", "Give one answer.");
  const value = values[0];
  if (typeof value !== "string") {
    // a value that is not a string is refused as the kind's own rule refuses a wrong one
    return q.type === "confirm"
      ? problem("not_offered", "Choose yes or no.")
      : problem("grammar", "Give the answer as text.");
  }
  switch (q.type) {
    case "confirm":
      return value === YES || value === NO
        ? undefined
        : problem("not_offered", "Choose yes or no.");
    case "number":
      return checkNumber(q, value);
    case "scale":
      return checkScale(q, value);
    case "date":
      return checkDate(q, value);
    default:
      return undefined;
  }
}

// ---- what JSON Schema cannot say about the members ----------------------------------------

/** The members each kind owns: a member of one kind is refused on another. */
const OWNED: Record<string, readonly (keyof ChoiceQuestion)[]> = {
  confirm: ["yesLabel", "noLabel"],
  number: ["minValue", "maxValue", "step", "unit"],
  scale: ["from", "to", "fromLabel", "toLabel"],
  date: ["minDate", "maxDate"],
};

/** A member of a kind that sits on a question of another type, as a refusal's reason. */
export function strayMember(q: ChoiceQuestion): string | undefined {
  for (const [kind, members] of Object.entries(OWNED)) {
    if (kind === q.type) continue;
    const found = members.find((m) => q[m] !== undefined);
    if (found !== undefined) {
      return `the question ${JSON.stringify(q.id)} is type "${q.type}" but has ${found}, which belongs to type "${kind}"`;
    }
  }
  return undefined;
}

/**
 * What JSON Schema cannot say about a question of `confirm`, `number`, `scale` or `date` (a rank is
 * judged with the options of the others): no options and none of `multiple`, `allowOther`, `min`
 * and `max`, `minValue` at most `maxValue`, a positive `step`, a `to` above `from`, real days in
 * order, and a `default` that is an answer the kind accepts.
 */
export function kindLimits(q: ChoiceQuestion): string | undefined {
  const at = `the question ${JSON.stringify(q.id)}`;
  const { multiple: declaredMultiple } = q.declared;
  if (q.options.length > 0 && q.type !== "rank") {
    return `${at} is type "${q.type}": it takes no options`;
  }
  if (declaredMultiple === true) return `${at} is type "${q.type}": it cannot have multiple: true`;
  if (q.allowOther) return `${at} is type "${q.type}": it cannot have allowOther`;
  if (q.min !== undefined || q.max !== undefined) {
    return `${at} is type "${q.type}": min and max count choices, which this kind has none of`;
  }
  switch (q.type) {
    case "number": {
      for (const member of ["minValue", "maxValue", "step"] as const) {
        const v = q[member];
        if (v !== undefined && parseDecimal(v) === undefined) {
          return `${at} has ${member} ${JSON.stringify(v)}, which is not a decimal (digits and a point, no sign on a zero)`;
        }
      }
      const min = q.minValue === undefined ? undefined : parseDecimal(q.minValue);
      const max = q.maxValue === undefined ? undefined : parseDecimal(q.maxValue);
      if (min !== undefined && max !== undefined && min > max) {
        return `the minValue of ${at} (${q.minValue}) is above its maxValue (${q.maxValue})`;
      }
      const step = q.step === undefined ? undefined : parseDecimal(q.step);
      if (step !== undefined && step <= 0n) return `the step of ${at} must be above zero`;
      break;
    }
    case "scale": {
      if (q.to === undefined) return `${at} is type "scale": it needs a "to"`;
      if (q.to <= scaleFrom(q)) {
        return `the "to" of ${at} (${q.to}) must be above its "from" (${scaleFrom(q)})`;
      }
      break;
    }
    case "date": {
      for (const member of ["minDate", "maxDate"] as const) {
        const v = q[member];
        if (v !== undefined && !isCalendarDate(v)) {
          return `${at} has ${member} ${JSON.stringify(v)}, which is not a day that exists (YYYY-MM-DD)`;
        }
      }
      if (q.minDate !== undefined && q.maxDate !== undefined && q.minDate > q.maxDate) {
        return `the minDate of ${at} (${q.minDate}) is above its maxDate (${q.maxDate})`;
      }
      break;
    }
    default:
      break;
  }
  // a default has the shape of its kind: a string, and for a rank a list
  if (q.defaultShape !== undefined && q.defaultShape !== (q.type === "rank" ? "list" : "string")) {
    return `the default of ${at} must be ${q.type === "rank" ? "a list of the options' values" : "a string"}`;
  }
  const given = q.defaultText !== undefined ? [q.defaultText] : q.defaults;
  if (given.length > 0) {
    const bad = checkValues(q, given);
    if (bad) return `the default of ${at} is not an answer it accepts: ${bad.message}`;
  }
  return undefined;
}
