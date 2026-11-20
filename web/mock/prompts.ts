/**
 * A person's prompt library, written from docs/api/prompts-v1.md and from nothing else: this file imports nothing of `web/src`,
 * because it is the SECOND implementation of the contract (the first is `orch_core::prompt`, in Rust). A third client would write
 * a third. `prompts.test.ts` holds both to the same vectors in docs/api/examples/prompts/ (and the catalog page's blocks).
 *
 * Two halves. The rules are pure functions: a body into segments, a whole save into its first error, an answer into its first
 * refusal, a prompt and an answer into text. The routes (`handlePrompts`) are a function of one session's shelf and the request,
 * with no I/O of its own (`server.ts` reads the body and writes the answer): a person is one session of the mock, as for every
 * other route; the prompts "of somebody else" a test puts there with `POST /__mock/prompts/shared` are the shared ones.
 *
 * An integer member is any JSON number whose value is an integer in range (`5`, `5.0` and `5e0` are 5), which is what
 * `Number.isInteger` after `JSON.parse` says and what the orchestrator reads (ADR 0070, after review).
 */
import { randomBytes } from "node:crypto";

// ---- the constants of the page ----------------------------------------------------------------

export const MAX_PROMPTS = 100;
export const MAX_NAME = 32;
export const MAX_TITLE = 120;
export const MAX_DESCRIPTION = 500;
export const MAX_BODY_BYTES = 16384;
export const MAX_VARIABLES = 8;
export const MAX_AUDIENCE = 32;
export const MAX_AUDIENCE_VALUE = 64;
export const MAX_RENDERED_BYTES = 32768;
export const MAX_REQUEST_BYTES = 65536;
export const PAGE = 100;

type Json = unknown;
type Rec = Record<string, Json>;
const isRec = (v: Json): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const scalars = (s: string): number => Array.from(s).length;
const bytes = (s: string): number => Buffer.byteLength(s, "utf8");

// ---- references -------------------------------------------------------------------------------

export type Segment = string | { ref: string };

const REF = /^([A-Za-z0-9_.:-]{1,64})\}\}/;

/** A body into its segments (literal text with the escapes resolved, and references), or the UTF-16 offset of the first bad `{{`. */
export function parseBody(body: string): { segments: Segment[] } | { offset: number } {
  const segments: Segment[] = [];
  let literal = "";
  const flush = () => {
    if (literal !== "") segments.push(literal);
    literal = "";
  };
  let i = 0;
  while (i < body.length) {
    const c = body[i] as string;
    if (c === "\\" && body.startsWith("{{", i + 1)) {
      literal += "{{";
      i += 3;
    } else if (c === "{" && body.startsWith("{{", i)) {
      const m = REF.exec(body.slice(i + 2, i + 2 + 64 + 2));
      if (!m) return { offset: i };
      flush();
      segments.push({ ref: m[1] as string });
      i += 2 + (m[0] as string).length;
    } else {
      literal += c;
      i += 1;
    }
  }
  flush();
  return { segments };
}

// ---- the variables ----------------------------------------------------------------------------

const ID = /^[A-Za-z0-9_.:-]{1,64}$/;
const DECIMAL = /^(-?)(0|[1-9][0-9]{0,14})(?:\.([0-9]{1,6}))?$/;
const MICRO = 1_000_000n;
const ICONS = new Set([
  "agent",
  "read",
  "edit",
  "delete",
  "move",
  "search",
  "execute",
  "think",
  "fetch",
  "web",
  "git",
  "test",
  "file",
  "tool",
  "opencode",
]);
const KINDS = [
  "single",
  "multiple",
  "select",
  "text",
  "longText",
  "confirm",
  "number",
  "scale",
  "date",
  "rank",
] as const;
type Kind = (typeof KINDS)[number];
/** Every member a question of the catalog's schema has (`show` is the catalog's too, and a prompt refuses it). */
const CATALOG_MEMBERS = new Set([
  "id",
  "question",
  "help",
  "show",
  "type",
  "options",
  "multiple",
  "allowOther",
  "required",
  "default",
  "min",
  "max",
  "yesLabel",
  "noLabel",
  "minValue",
  "maxValue",
  "step",
  "unit",
  "from",
  "to",
  "fromLabel",
  "toLabel",
  "minDate",
  "maxDate",
]);
const COMMON = ["id", "question", "help", "type", "required"];
const KIND_MEMBERS: Record<Kind, string[]> = {
  single: [...COMMON, "options", "allowOther", "default", "multiple"],
  multiple: [...COMMON, "options", "allowOther", "default", "multiple", "min", "max"],
  select: [...COMMON, "options", "allowOther", "default", "multiple", "min", "max"],
  text: [...COMMON, "default"],
  longText: [...COMMON, "default"],
  confirm: [...COMMON, "yesLabel", "noLabel", "default"],
  number: [...COMMON, "minValue", "maxValue", "step", "unit", "default"],
  scale: [...COMMON, "from", "to", "fromLabel", "toLabel", "default"],
  date: [...COMMON, "minDate", "maxDate", "default"],
  rank: [...COMMON, "options", "default"],
};
const OPTION_MEMBERS = ["value", "label", "description", "icon", "recommended"];

/** A decimal string as millionths, or undefined (zero has no sign). */
function decimal(text: Json): bigint | undefined {
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

/** A save refused: where, and for a reference the UTF-16 offset. */
export type SaveError = { code: string; at: string; offset?: number };
const fail = (code: string, at: string, offset?: number): SaveError => ({
  code,
  at,
  ...(offset === undefined ? {} : { offset }),
});

const intIn = (v: Json, lo: number, hi: number): v is number =>
  typeof v === "number" && Number.isInteger(v) && v >= lo && v <= hi;
const textIn = (v: Json, lo: number, hi: number): v is string =>
  typeof v === "string" && scalars(v) >= lo && scalars(v) <= hi;
/** {@link textIn} for a text of a variable that a person reads: and nothing {@link unsafeText} refuses. */
const shownIn = (v: Json, lo: number, hi: number): v is string =>
  textIn(v, lo, hi) && !unsafeText(v);
/** Whether `s` has a control character (U+0000 to U+001F, U+007F to U+009F) other than the `allowed` ones. */
function hasControl(s: string, allowed = ""): boolean {
  for (const ch of s) {
    const c = ch.codePointAt(0) ?? 0;
    if ((c <= 0x1f || (c >= 0x7f && c <= 0x9f)) && !allowed.includes(ch)) return true;
  }
  return false;
}
/** Whether `c` is a bidirectional embedding, override or isolate control, or a line or paragraph separator (U+202A to U+202E, U+2066 to U+2069, U+2028, U+2029). */
const isBidi = (c: number): boolean =>
  (c >= 0x202a && c <= 0x202e) || (c >= 0x2066 && c <= 0x2069) || c === 0x2028 || c === 0x2029;
/** A text a person reads (a title, a description, a question, a label): no control character but the `allowed` ones, and no bidirectional control or separator. */
function unsafeText(s: string, allowed = ""): boolean {
  return hasControl(s, allowed) || Array.from(s).some((ch) => isBidi(ch.codePointAt(0) ?? 0));
}

const optionsOf = (q: Rec): Rec[] => (Array.isArray(q.options) ? (q.options as Rec[]) : []);
const kindOf = (q: Rec): Kind => (q.type === undefined ? "single" : (q.type as Kind));
/** Whether several choices may be taken. */
const takesSeveral = (q: Rec): boolean =>
  kindOf(q) === "multiple" || (kindOf(q) === "select" && q.multiple === true);
const isChoice = (k: Kind): boolean => k === "single" || k === "multiple" || k === "select";

/** The first rule a variable breaks (`at` is relative to the variable: `variables.<i>` is put in front by the caller), or undefined. */
function checkVariable(i: number, q: Json, seen: Set<string>): SaveError | undefined {
  const here = `variables.${i}`;
  if (!isRec(q)) return fail("bad_variable", here);
  // (2) members that no question has, and show
  for (const k of Object.keys(q).sort()) {
    if (!CATALOG_MEMBERS.has(k) || k === "show") return fail("bad_variable", `${here}.${k}`);
  }
  // (3) the type
  if (q.type !== undefined && !(KINDS as readonly unknown[]).includes(q.type)) {
    return fail("bad_variable", `${here}.type`);
  }
  const kind = kindOf(q);
  // (4) members this kind does not take
  const takes = new Set(KIND_MEMBERS[kind]);
  const single = isChoice(kind) && !takesSeveral(q);
  for (const k of Object.keys(q).sort()) {
    if (!takes.has(k) || (single && (k === "min" || k === "max"))) {
      return fail("bad_variable", `${here}.${k}`);
    }
  }
  // (5) each member's own type and bounds
  if (typeof q.id !== "string" || !ID.test(q.id) || seen.has(q.id)) {
    return fail("bad_variable", `${here}.id`);
  }
  seen.add(q.id);
  if (!shownIn(q.question, 1, 300)) return fail("bad_variable", `${here}.question`);
  if (q.help !== undefined && !shownIn(q.help, 0, 300)) return fail("bad_variable", `${here}.help`);
  if (q.required !== undefined && typeof q.required !== "boolean") {
    return fail("bad_variable", `${here}.required`);
  }
  const hasOptions = kind === "rank" || isChoice(kind);
  if (hasOptions) {
    if (!Array.isArray(q.options)) return fail("bad_variable", `${here}.options`);
    for (const [j, o] of q.options.entries()) {
      const at = `${here}.options.${j}`;
      if (!isRec(o)) return fail("bad_variable", at);
      for (const k of Object.keys(o).sort()) {
        if (!OPTION_MEMBERS.includes(k)) return fail("bad_variable", `${at}.${k}`);
      }
      if (typeof o.value !== "string" || !ID.test(o.value)) {
        return fail("bad_variable", `${at}.value`);
      }
      if (!shownIn(o.label, 1, 120)) return fail("bad_variable", `${at}.label`);
      if (o.description !== undefined && !shownIn(o.description, 0, 300)) {
        return fail("bad_variable", `${at}.description`);
      }
      if (o.icon !== undefined && !(typeof o.icon === "string" && ICONS.has(o.icon))) {
        return fail("bad_variable", `${at}.icon`);
      }
      if (o.recommended !== undefined && typeof o.recommended !== "boolean") {
        return fail("bad_variable", `${at}.recommended`);
      }
    }
  }
  if (q.multiple !== undefined && typeof q.multiple !== "boolean") {
    return fail("bad_variable", `${here}.multiple`);
  }
  if (q.allowOther !== undefined && typeof q.allowOther !== "boolean") {
    return fail("bad_variable", `${here}.allowOther`);
  }
  if (q.default !== undefined) {
    const listOk = hasOptions;
    const ok =
      typeof q.default === "string" ||
      (listOk && Array.isArray(q.default) && q.default.every((v) => typeof v === "string"));
    if (!ok) return fail("bad_variable", `${here}.default`);
  }
  if (q.min !== undefined && !intIn(q.min, 0, 50)) return fail("bad_variable", `${here}.min`);
  if (q.max !== undefined && !intIn(q.max, 1, 50)) return fail("bad_variable", `${here}.max`);
  for (const k of ["yesLabel", "noLabel"]) {
    if (q[k] !== undefined && !shownIn(q[k], 1, 40)) return fail("bad_variable", `${here}.${k}`);
  }
  for (const k of ["minValue", "maxValue", "step"]) {
    if (q[k] !== undefined && decimal(q[k]) === undefined)
      return fail("bad_variable", `${here}.${k}`);
  }
  if (q.unit !== undefined && !shownIn(q.unit, 1, 20)) return fail("bad_variable", `${here}.unit`);
  if (q.from !== undefined && !intIn(q.from, 0, 1)) return fail("bad_variable", `${here}.from`);
  if (kind === "scale" && !intIn(q.to, 2, 10)) return fail("bad_variable", `${here}.to`);
  for (const k of ["fromLabel", "toLabel"]) {
    if (q[k] !== undefined && !shownIn(q[k], 1, 60)) return fail("bad_variable", `${here}.${k}`);
  }
  for (const k of ["minDate", "maxDate"]) {
    if (q[k] !== undefined && !(typeof q[k] === "string" && realDay(q[k] as string))) {
      return fail("bad_variable", `${here}.${k}`);
    }
  }
  // (6) the rules between members
  if (kind === "single" && q.multiple === true) return fail("bad_variable", `${here}.multiple`);
  if (kind === "multiple" && q.multiple === false) return fail("bad_variable", `${here}.multiple`);
  if (hasOptions) {
    const n = optionsOf(q).length;
    const [lo, hi] = kind === "select" ? [2, 50] : [2, 12];
    if (n < lo || n > hi) return fail("bad_variable", `${here}.options`);
    const values = new Set<string>();
    for (const [j, o] of optionsOf(q).entries()) {
      if (values.has(o.value as string)) return fail("bad_variable", `${here}.options.${j}.value`);
      values.add(o.value as string);
    }
    if (takesSeveral(q)) {
      const offered = n + (q.allowOther === true ? 1 : 0);
      const min = q.min as number | undefined;
      const max = q.max as number | undefined;
      if (min !== undefined && max !== undefined && min > max) {
        return fail("bad_variable", `${here}.min`);
      }
      if (min !== undefined && min > offered) return fail("bad_variable", `${here}.min`);
      if (max !== undefined && max > offered) return fail("bad_variable", `${here}.max`);
    }
  }
  const [minV, maxV, step] = [decimal(q.minValue), decimal(q.maxValue), decimal(q.step)];
  if (minV !== undefined && maxV !== undefined && minV > maxV) {
    return fail("bad_variable", `${here}.minValue`);
  }
  if (step !== undefined && step <= 0n) return fail("bad_variable", `${here}.step`);
  if (typeof q.minDate === "string" && typeof q.maxDate === "string" && q.minDate > q.maxDate) {
    return fail("bad_variable", `${here}.minDate`);
  }
  if (kind === "scale") {
    const from = typeof q.from === "number" ? q.from : 1;
    if ((q.to as number) <= from) return fail("bad_variable", `${here}.to`);
  }
  if (q.default !== undefined && !defaultAccepted(q))
    return fail("bad_variable", `${here}.default`);
  return undefined;
}

/** A default is an answer the kind accepts (never an empty rank). */
function defaultAccepted(q: Rec): boolean {
  const kind = kindOf(q);
  const d = q.default;
  const loose = { ...q, required: false };
  if (kind === "text" || kind === "longText") {
    return (
      typeof d === "string" &&
      !Array.from(d).some((ch) => isBidi(ch.codePointAt(0) ?? 0)) &&
      refusalOfEntry(loose, { id: "d", values: [], text: d }) === undefined
    );
  }
  const values = Array.isArray(d) ? d : [d];
  if (kind === "rank" && values.length === 0) return false;
  if (kind === "single" || (kind === "select" && q.multiple !== true)) {
    if (values.length > 1) return false;
  }
  return refusalOfEntry(loose, { id: "d", values }) === undefined;
}

// ---- the whole save ---------------------------------------------------------------------------

export type Save = {
  name: string;
  title: string;
  description: string;
  body: string;
  variables: Json[];
  audience: string[];
};

const AUDIENCE_VALUE = /^[!-~]{1,64}$/;

/** The audience as a set: each value once, sorted by bytes. */
export const audienceSet = (audience: string[]): string[] => [...new Set(audience)].sort();

/** The first rule a save breaks, in the order of the page, or undefined. */
export function validateSave(s: Save): SaveError | undefined {
  if (!/^[a-z0-9][a-z0-9-]{0,31}$/.test(s.name)) return fail("bad_name", "name");
  if (
    !textIn(s.title, 1, MAX_TITLE) ||
    unsafeText(s.title) ||
    /^[ \t\n\r]|[ \t\n\r]$/.test(s.title)
  ) {
    return fail("bad_title", "title");
  }
  if (scalars(s.description) > MAX_DESCRIPTION || unsafeText(s.description, "\t\n")) {
    return fail("bad_description", "description");
  }
  if (
    bytes(s.body) < 1 ||
    bytes(s.body) > MAX_BODY_BYTES ||
    hasControl(s.body, "\t\n\r") ||
    /^[ \t\n\r]*$/.test(s.body)
  ) {
    return fail("bad_body", "body");
  }
  const parsed = parseBody(s.body);
  if ("offset" in parsed) return fail("bad_reference", "body", parsed.offset);
  if (s.variables.length > MAX_VARIABLES) return fail("bad_variable", "variables");
  const seen = new Set<string>();
  for (const [i, q] of s.variables.entries()) {
    const e = checkVariable(i, q, seen);
    if (e) return e;
  }
  // the first reference that no variable declares, then the first variable that no reference uses
  const used = new Set<string>();
  for (const seg of parsed.segments) {
    if (typeof seg === "string") continue;
    if (!seen.has(seg.ref)) {
      return fail("undeclared_variable", "body", offsetOfReference(s.body, seen));
    }
    used.add(seg.ref);
  }
  for (const [i, q] of s.variables.entries()) {
    if (!used.has((q as Rec).id as string)) return fail("unused_variable", `variables.${i}`);
  }
  const set = new Set(s.audience);
  if (set.size > MAX_AUDIENCE) return fail("bad_audience", "audience");
  for (const v of set) if (!AUDIENCE_VALUE.test(v)) return fail("bad_audience", "audience");
  if (set.has("*") && set.size > 1) return fail("bad_audience", "audience");
  return undefined;
}

/** The UTF-16 offset of the `{{` of the first reference of `body` whose id is not in `declared`. */
function offsetOfReference(body: string, declared: Set<string>): number {
  let i = 0;
  while (i < body.length) {
    if (body[i] === "\\" && body.startsWith("{{", i + 1)) i += 3;
    else if (body[i] === "{" && body.startsWith("{{", i)) {
      const m = REF.exec(body.slice(i + 2, i + 2 + 64 + 2)) as RegExpExecArray;
      if (!declared.has(m[1] as string)) return i;
      i += 2 + (m[0] as string).length;
    } else i += 1;
  }
  return 0;
}

// ---- an answer --------------------------------------------------------------------------------

export type Refusal = { question: string; rule: string };

const labelOf = (q: Rec, value: Json): string =>
  String(optionsOf(q).find((o) => o.value === value)?.label ?? "");

/** The first rule that one entry breaks for one question, or undefined. `entry` is the object of the answer. */
function refusalOfEntry(q: Rec, entry: Rec): { rule: string } | undefined {
  const kind = kindOf(q);
  const required = q.required !== false;
  const values = entry.values;
  if (!Array.isArray(values)) return { rule: "unknown" };
  const allowed = new Set(["id", "values"]);
  if (kind === "text" || kind === "longText") allowed.add("text");
  else if (isChoice(kind) && q.allowOther === true) allowed.add("other");
  if (Object.keys(entry).some((k) => !allowed.has(k))) return { rule: "unexpected" };
  const v = values as Json[];
  switch (kind) {
    case "text":
    case "longText": {
      if (v.length > 0) return { rule: "not_offered" };
      if (entry.text === undefined) return required ? { rule: "required" } : undefined;
      const t = entry.text;
      if (typeof t !== "string" || t === "") return { rule: "grammar" };
      if (hasControl(t, kind === "text" ? "\t" : "\t\n")) return { rule: "grammar" };
      if (scalars(t) > (kind === "text" ? 500 : 2000)) return { rule: "too_long" };
      return undefined;
    }
    case "single":
    case "multiple":
    case "select":
      return refusalOfChoice(q, entry, v, required);
    default:
      return refusalOfKind(q, v, required);
  }
}

function refusalOfChoice(
  q: Rec,
  entry: Rec,
  v: Json[],
  required: boolean,
): { rule: string } | undefined {
  const other = entry.other;
  const count = v.length + (other !== undefined ? 1 : 0);
  if (count === 0) return required ? { rule: "required" } : undefined;
  const several = takesSeveral(q);
  if (!several && count > 1) return { rule: "too_many" };
  const options = optionsOf(q).map((o) => o.value);
  if (v.some((x) => typeof x !== "string" || !options.includes(x))) return { rule: "not_offered" };
  if (new Set(v).size !== v.length) return { rule: "repeated" };
  const places = v.map((x) => options.indexOf(x));
  if (places.some((p, i) => i > 0 && p < (places[i - 1] as number)))
    return { rule: "out_of_order" };
  if (other !== undefined) {
    if (typeof other !== "string" || other === "" || hasControl(other, "\t")) {
      return { rule: "grammar" };
    }
    if (scalars(other) > 500) return { rule: "too_long" };
  }
  if (several) {
    if (typeof q.max === "number" && count > q.max) return { rule: "too_many" };
    if (typeof q.min === "number" && count < q.min) return { rule: "too_few" };
  }
  return undefined;
}

/** The rules of the five newer kinds (the catalog's table). */
function refusalOfKind(q: Rec, v: Json[], required: boolean): { rule: string } | undefined {
  if (v.length === 0) return required ? { rule: "required" } : undefined;
  if (q.type === "rank") {
    const options = optionsOf(q).map((o) => o.value);
    if (v.some((x) => !options.includes(x))) return { rule: "not_offered" };
    if (new Set(v).size !== v.length || v.length !== options.length) {
      return { rule: "not_a_permutation" };
    }
    return undefined;
  }
  if (v.length > 1) return { rule: "too_many" };
  const value = v[0];
  if (typeof value !== "string") return { rule: q.type === "confirm" ? "not_offered" : "grammar" };
  switch (q.type) {
    case "confirm":
      return value === "yes" || value === "no" ? undefined : { rule: "not_offered" };
    case "number": {
      const n = decimal(value);
      if (n === undefined) return { rule: "grammar" };
      const [min, max, step] = [decimal(q.minValue), decimal(q.maxValue), decimal(q.step)];
      if (min !== undefined && n < min) return { rule: "below_min" };
      if (max !== undefined && n > max) return { rule: "above_max" };
      if (step !== undefined && step > 0n && (n - (min ?? 0n)) % step !== 0n) {
        return { rule: "off_step" };
      }
      return undefined;
    }
    case "scale": {
      if (!/^(0|[1-9][0-9]?)$/.test(value)) return { rule: "grammar" };
      const from = typeof q.from === "number" ? q.from : 1;
      const to = typeof q.to === "number" ? q.to : from;
      if (Number(value) < from) return { rule: "below_min" };
      if (Number(value) > to) return { rule: "above_max" };
      return undefined;
    }
    case "date": {
      if (!realDay(value)) return { rule: "grammar" };
      if (typeof q.minDate === "string" && value < q.minDate) return { rule: "below_min" };
      if (typeof q.maxDate === "string" && value > q.maxDate) return { rule: "above_max" };
      return undefined;
    }
    default:
      return undefined;
  }
}

/** The first refusal of a render request's answers against the variables, or undefined. `note` says the request had one. */
export function validateAnswers(
  variables: Rec[],
  answers: Json,
  note: boolean,
): Refusal | undefined {
  if (!Array.isArray(answers)) return { question: "*", rule: "unknown" };
  if (note) return { question: "*", rule: "unexpected" };
  if (
    answers.length !== variables.length ||
    answers.some((a, i) => !isRec(a) || a.id !== (variables[i] as Rec).id)
  ) {
    return { question: "*", rule: "unknown" };
  }
  for (const [i, q] of variables.entries()) {
    const refusal = refusalOfEntry(q, answers[i] as Rec);
    if (refusal) return { question: q.id as string, rule: refusal.rule };
  }
  return undefined;
}

// ---- render -----------------------------------------------------------------------------------

/** What one answer renders as. The answer has passed {@link validateAnswers}. */
function valueText(q: Rec, entry: Rec): string {
  const kind = kindOf(q);
  const values = entry.values as Json[];
  switch (kind) {
    case "text":
    case "longText":
      return typeof entry.text === "string" ? entry.text : "";
    case "single":
    case "multiple":
    case "select": {
      const parts = values.map((v) => labelOf(q, v));
      if (typeof entry.other === "string") parts.push(entry.other);
      return parts.join(", ");
    }
    case "rank":
      return values.map((v, i) => `${i + 1}. ${labelOf(q, v)}`).join("; ");
    default:
      return values.length === 0 ? "" : String(values[0]);
  }
}

/** The text of a prompt for validated answers, or `too_long` (decided before anything is built). */
export function renderText(
  body: string,
  variables: Rec[],
  answers: Rec[],
): { text: string } | { tooLong: true } {
  const parsed = parseBody(body);
  if (!("segments" in parsed)) return { tooLong: true };
  const text = new Map<string, string>();
  for (const [i, q] of variables.entries()) {
    text.set(q.id as string, valueText(q, answers[i] as Rec));
  }
  let size = 0;
  for (const seg of parsed.segments) {
    size += bytes(typeof seg === "string" ? seg : (text.get(seg.ref) ?? ""));
  }
  if (size > MAX_RENDERED_BYTES) return { tooLong: true };
  return {
    text: parsed.segments
      .map((seg) => (typeof seg === "string" ? seg : (text.get(seg.ref) ?? "")))
      .join(""),
  };
}

// ---- the routes -------------------------------------------------------------------------------

/** An answer: the status, the body and the headers beyond the content type. */
export type Reply = {
  status: number;
  body?: unknown;
  type?: string;
  headers?: Record<string, string>;
};

const problem = (
  status: number,
  title: string,
  detail?: string,
  code?: string,
  extra: Rec = {},
): Reply => ({
  status,
  type: "application/problem+json",
  body: { title, status, ...(detail ? { detail } : {}), ...(code ? { code } : {}), ...extra },
});
const NOT_FOUND = problem(404, "Not found");
const bad = (detail: string, code = "bad_request"): Reply =>
  problem(400, "Bad Request", detail, code);
const SAVE_WORDS: Record<string, string> = {
  bad_name: "the name is not 1 to 32 of a-z, 0-9 and -, starting with a letter or a digit",
  bad_title: "the title is not 1 to 120 characters without controls or edge spaces",
  bad_description: "the description is not at most 500 characters without controls",
  bad_body: "the body is not 1 to 16384 bytes of text without controls",
  bad_reference: "the body has a {{ that is not a reference",
  bad_variable: "a variable is not a question the library takes",
  undeclared_variable: "the body names a variable that is not declared",
  unused_variable: "a declared variable is not used by the body",
  bad_audience: "the audience is not up to 32 roles, or * alone",
};
const saveProblem = (e: SaveError): Reply =>
  problem(400, "Bad Request", SAVE_WORDS[e.code], e.code, {
    at: e.at,
    ...(e.offset === undefined ? {} : { offset: e.offset }),
  });
const ANSWER_WORDS = "the answer to this question was not used";

/** A prompt a person owns, or one of somebody else's shared with roles. */
type Stored = {
  id: string;
  save: Save;
  revision: number;
  updatedAt: string;
};

/** One session's library: the person's own prompts and the shared ones of "somebody else". */
export type Shelf = {
  own: Map<string, Stored>;
  others: Map<string, Stored>;
  /** One sequence for every write of the session: a revision is never given twice. */
  revision: number;
};
export const newShelf = (): Shelf => ({ own: new Map(), others: new Map(), revision: 0 });

const newId = (taken: (id: string) => boolean): string => {
  for (;;) {
    let n = BigInt(`0x${randomBytes(8).toString("hex")}`);
    let s = "";
    for (let i = 0; i < 12; i++) {
      s = "0123456789abcdefghijklmnopqrstuvwxyz"[Number(n % 36n)] + s;
      n /= 36n;
    }
    if (!taken(`p-${s}`)) return `p-${s}`;
  }
};

/** What the routes know about the caller. */
export type Caller = { roles: string[]; canShare: boolean };

/** A request, with its body as text. */
export type PromptRequest = {
  method: string;
  /** The path after `/api/prompts`: `""`, `"/p-…"` or `"/p-…/render"`. */
  rest: string;
  search: URLSearchParams;
  contentType: string | undefined;
  body: string;
};

const ID_PATH = /^\/(p-[a-z0-9]{12})(\/render)?$/;
const PROMPT_ID = /^p-[a-z0-9]{12}$/;
const REVISION = /^[1-9][0-9]{0,18}$/;

const view = (s: Stored, own: boolean) => ({
  id: s.id,
  name: s.save.name,
  title: s.save.title,
  description: s.save.description,
  body: s.save.body,
  variables: s.save.variables,
  own,
  ...(own ? { audience: audienceSet(s.save.audience) } : {}),
  revision: String(s.revision),
  updatedAt: s.updatedAt,
});
const summary = (s: Stored, own: boolean) => ({
  id: s.id,
  name: s.save.name,
  title: s.save.title,
  description: s.save.description,
  variableCount: s.save.variables.length,
  own,
  ...(own ? { audience: audienceSet(s.save.audience) } : {}),
  revision: String(s.revision),
  updatedAt: s.updatedAt,
});

/** The members of a body that is one JSON object: each key once (in every object inside it too), strings well formed, nothing `null`. */
function readObject(body: string, members: readonly string[]): Rec | undefined {
  let value: Json;
  try {
    value = JSON.parse(body);
  } catch {
    return undefined;
  }
  if (!isRec(value)) return undefined;
  if (hasDuplicateKey(body)) return undefined;
  if (Object.keys(value).some((k) => !members.includes(k))) return undefined;
  if (!wellFormed(value)) return undefined;
  return value;
}

const wellFormed = (v: Json): boolean => {
  if (v === null) return true;
  if (typeof v === "string") return v.isWellFormed();
  if (Array.isArray(v)) return v.every(wellFormed);
  if (isRec(v)) return Object.entries(v).every(([k, x]) => k.isWellFormed() && wellFormed(x));
  return true;
};

/** `null` is the wrong type everywhere, but a null INSIDE a variable is that variable's own refusal: only top-level members are held to it. */
function nullFree(o: Rec): boolean {
  return Object.values(o).every((v) => v !== null);
}

/**
 * Whether any object of a JSON text that has parsed, at any depth (the request's own, a variable, an option, an answer), has a key twice.
 * `JSON.parse` keeps the last of two, which is not what the person wrote.
 */
function hasDuplicateKey(text: string): boolean {
  // one entry per open container: the keys of an object, or undefined for an array
  const stack: (Set<string> | undefined)[] = [];
  let expectKey = false;
  for (const m of text.matchAll(/"(?:[^"\\]|\\.)*"|[{}[\],:]|[^\s{}[\],:"]+/g)) {
    const token = m[0];
    if (token === "{") {
      stack.push(new Set());
      expectKey = true;
    } else if (token === "[") {
      stack.push(undefined);
      expectKey = false;
    } else if (token === "}" || token === "]") {
      stack.pop();
      expectKey = false;
    } else if (token === ",") expectKey = stack.at(-1) !== undefined;
    else if (token === ":") expectKey = false;
    else if (expectKey && token.startsWith('"')) {
      const keys = stack.at(-1) as Set<string>;
      const key = JSON.parse(token) as string;
      if (keys.has(key)) return true;
      keys.add(key);
      expectKey = false;
    }
  }
  return false;
}

const JSON_TYPE = (contentType: string | undefined): boolean =>
  (contentType ?? "").split(";")[0]?.trim().toLowerCase() === "application/json";

/** The `Save` a shape-checked body makes, with `base` for the members it leaves out; or undefined for a member of the wrong type. */
function saveOf(o: Rec, base?: Save): Save | undefined {
  const text = (k: string): string | undefined | null =>
    o[k] === undefined ? undefined : typeof o[k] === "string" ? (o[k] as string) : null;
  const [name, title, description, body] = [
    text("name"),
    text("title"),
    text("description"),
    text("body"),
  ];
  if ([name, title, description, body].includes(null)) return undefined;
  if (o.variables !== undefined && !Array.isArray(o.variables)) return undefined;
  if (
    o.audience !== undefined &&
    !(Array.isArray(o.audience) && o.audience.every((v) => typeof v === "string"))
  ) {
    return undefined;
  }
  return {
    name: (name ?? base?.name) as string,
    title: (title ?? base?.title) as string,
    description: description ?? base?.description ?? "",
    body: (body ?? base?.body) as string,
    variables: (o.variables as Json[] | undefined) ?? base?.variables ?? [],
    audience: (o.audience as string[] | undefined) ?? base?.audience ?? [],
  };
}

const sameSave = (a: Save, b: Save): boolean =>
  JSON.stringify([a.name, a.title, a.description, a.body, a.variables, audienceSet(a.audience)]) ===
  JSON.stringify([b.name, b.title, b.description, b.body, b.variables, audienceSet(b.audience)]);

const FORBIDDEN = problem(403, "Forbidden", "your roles do not grant prompts.share", "forbidden");
const STALE = problem(
  409,
  "Conflict",
  "the prompt changed since the revision sent",
  "stale_revision",
);

/** Whether `shared` is seen by a caller with these roles: `*`, or one of their roles, byte for byte. */
const sees = (audience: string[], roles: string[]): boolean =>
  audience.includes("*") || audience.some((a) => roles.includes(a));

/** Every route of `/api/prompts`, as a function of the session's shelf. The identity layer and `no_access` have been answered. */
export function handlePrompts(
  shelf: Shelf,
  caller: Caller,
  request: PromptRequest,
  now: Date,
): Reply | undefined {
  const { method, rest } = request;
  const visible = (id: string): { stored: Stored; own: boolean } | undefined => {
    const own = shelf.own.get(id);
    if (own) return { stored: own, own: true };
    const other = shelf.others.get(id);
    if (other && sees(other.save.audience, caller.roles)) return { stored: other, own: false };
    return undefined;
  };

  if (rest === "") {
    if (method === "GET") return list(shelf, caller, request.search);
    if (method === "POST") return create(shelf, caller, request, now);
    return undefined;
  }
  const route = ID_PATH.exec(rest);
  if (!route) {
    // `/api/prompts/<anything>`: a path that is not an id is a 400, for the verbs the routes have
    const bare = /^\/[^/]+(\/render)?$/.exec(rest);
    const verbs = bare?.[1] ? ["POST"] : ["GET", "PATCH", "DELETE"];
    if (!bare || !verbs.includes(method)) return undefined;
    if ((method === "POST" || method === "PATCH") && !JSON_TYPE(request.contentType)) {
      return problem(415, "Unsupported Media Type", "the body is application/json");
    }
    return bad("the prompt id is p- and 12 of a-z, 0-9");
  }
  const id = route[1] as string;
  if (route[2]) return method === "POST" ? render(shelf, caller, request, id, visible) : undefined;
  if (method === "GET") {
    const found = visible(id);
    return found ? { status: 200, body: view(found.stored, found.own) } : NOT_FOUND;
  }
  if (method === "PATCH") return update(shelf, caller, request, id, now);
  if (method === "DELETE") return remove(shelf, request, id);
  return undefined;
}

function list(shelf: Shelf, caller: Caller, search: URLSearchParams): Reply {
  const scope = search.get("scope") ?? "all";
  if (!["all", "mine", "shared"].includes(scope))
    return bad("scope is all, mine or shared", "bad_scope");
  const cursor = search.get("cursor");
  if (cursor !== null && !PROMPT_ID.test(cursor)) {
    return bad("the cursor is the nextCursor of a page", "bad_cursor");
  }
  const rows = [
    ...(scope === "shared" ? [] : [...shelf.own.values()].map((s) => ({ s, own: true }))),
    ...(scope === "mine"
      ? []
      : [...shelf.others.values()]
          .filter((s) => sees(s.save.audience, caller.roles))
          .map((s) => ({ s, own: false }))),
  ]
    .filter((r) => cursor === null || r.s.id > cursor)
    .sort((a, b) => (a.s.id < b.s.id ? -1 : a.s.id > b.s.id ? 1 : 0));
  const page = rows.slice(0, PAGE);
  const last = page.at(-1);
  return {
    status: 200,
    body: {
      limit: PAGE,
      held: shelf.own.size,
      items: page.map((r) => summary(r.s, r.own)),
      ...(rows.length > PAGE && last ? { nextCursor: last.s.id } : {}),
    },
  };
}

const CREATE_MEMBERS = ["name", "title", "description", "body", "variables", "audience"];

function create(shelf: Shelf, caller: Caller, request: PromptRequest, now: Date): Reply {
  if (!JSON_TYPE(request.contentType))
    return problem(415, "Unsupported Media Type", "the body is application/json");
  if (bytes(request.body) > MAX_REQUEST_BYTES) return bad("the request is too large");
  const o = readObject(request.body, CREATE_MEMBERS);
  if (
    !o ||
    !nullFree(o) ||
    typeof o.name !== "string" ||
    typeof o.title !== "string" ||
    typeof o.body !== "string"
  ) {
    return bad(
      "the body is a prompt: name, title, body, and optionally description, variables, audience",
    );
  }
  const save = saveOf(o);
  if (!save) return bad("a member of the prompt has the wrong type");
  if (save.audience.length > 0 && !caller.canShare) return FORBIDDEN;
  const refused = validateSave(save);
  if (refused) return saveProblem(refused);
  const stored = store(shelf, save, now);
  if ("reply" in stored) return stored.reply;
  return { status: 201, body: view(stored.stored, true) };
}

function store(shelf: Shelf, save: Save, now: Date): { stored: Stored } | { reply: Reply } {
  if ([...shelf.own.values()].some((s) => s.save.name === save.name)) {
    return {
      reply: problem(409, "Conflict", "you already have a prompt of that name", "name_taken"),
    };
  }
  if (shelf.own.size >= MAX_PROMPTS) {
    return {
      reply: problem(409, "Conflict", "you hold the most prompts there are", "too_many_prompts"),
    };
  }
  const id = newId((x) => shelf.own.has(x) || shelf.others.has(x));
  shelf.revision += 1;
  const stored: Stored = { id, save, revision: shelf.revision, updatedAt: now.toISOString() };
  shelf.own.set(id, stored);
  return { stored };
}

const UPDATE_MEMBERS = ["revision", ...CREATE_MEMBERS];

function update(
  shelf: Shelf,
  caller: Caller,
  request: PromptRequest,
  id: string,
  now: Date,
): Reply {
  if (!JSON_TYPE(request.contentType))
    return problem(415, "Unsupported Media Type", "the body is application/json");
  if (bytes(request.body) > MAX_REQUEST_BYTES) return bad("the request is too large");
  const o = readObject(request.body, UPDATE_MEMBERS);
  if (!o || !nullFree(o) || typeof o.revision !== "string" || !REVISION.test(o.revision)) {
    return bad("the body is the revision last read and the members to change");
  }
  if (Object.keys(o).length < 2) return bad("there is nothing to change");
  const was = shelf.own.get(id);
  const save = was
    ? saveOf(o, was.save)
    : saveOf(o, { name: "a", title: "a", description: "", body: "a", variables: [], audience: [] });
  if (!save) return bad("a member of the prompt has the wrong type");
  if (!was) return NOT_FOUND;
  if (audienceSet(save.audience).length > 0 && !caller.canShare) return FORBIDDEN;
  if (String(was.revision) !== o.revision) return STALE;
  const refused = validateSave(save);
  if (refused) return saveProblem(refused);
  if (sameSave(was.save, save)) return { status: 200, body: view(was, true) };
  if (
    save.name !== was.save.name &&
    [...shelf.own.values()].some((s) => s.save.name === save.name)
  ) {
    return problem(409, "Conflict", "you already have a prompt of that name", "name_taken");
  }
  shelf.revision += 1;
  const next: Stored = { id, save, revision: shelf.revision, updatedAt: now.toISOString() };
  shelf.own.set(id, next);
  return { status: 200, body: view(next, true) };
}

function remove(shelf: Shelf, request: PromptRequest, id: string): Reply {
  const revision = request.search.get("revision");
  if (revision !== null && !REVISION.test(revision)) return bad("the revision is digits");
  const was = shelf.own.get(id);
  if (!was) return NOT_FOUND;
  if (revision !== null && String(was.revision) !== revision) return STALE;
  shelf.own.delete(id);
  return { status: 204 };
}

const RENDER_MEMBERS = ["revision", "answers", "note"];

function render(
  _shelf: Shelf,
  _caller: Caller,
  request: PromptRequest,
  id: string,
  visible: (id: string) => { stored: Stored; own: boolean } | undefined,
): Reply {
  if (!JSON_TYPE(request.contentType))
    return problem(415, "Unsupported Media Type", "the body is application/json");
  if (bytes(request.body) > MAX_REQUEST_BYTES) return bad("the request is too large");
  const o = readObject(request.body, RENDER_MEMBERS);
  if (!o || !nullFree(o)) return bad("the body is the answers, and optionally the revision");
  if (o.revision !== undefined && !(typeof o.revision === "string" && REVISION.test(o.revision))) {
    return bad("the revision is digits");
  }
  const found = visible(id);
  if (!found) return NOT_FOUND;
  const { stored } = found;
  if (o.revision !== undefined && String(stored.revision) !== o.revision) return STALE;
  const variables = stored.save.variables as Rec[];
  const refused = validateAnswers(variables, o.answers, "note" in o);
  if (refused) {
    return problem(422, "Unprocessable Content", ANSWER_WORDS, "invalid_answer", {
      question: refused.question,
      rule: refused.rule,
    });
  }
  const done = renderText(stored.save.body, variables, o.answers as Rec[]);
  if ("tooLong" in done) {
    return problem(
      422,
      "Unprocessable Content",
      "the text would be over 32768 bytes",
      "rendered_too_long",
    );
  }
  return { status: 200, body: { text: done.text, revision: String(stored.revision) } };
}

/**
 * `POST /__mock/prompts/shared`: prompts "of somebody else" that a test puts in the session, each `{name, title, description?, body,
 * variables?, audience}`. Which of them the session's roles see is the audience rule. A refusal is a sentence for the test.
 */
export function addShared(shelf: Shelf, body: Json, now: Date): string | undefined {
  if (!Array.isArray(body)) return "the body is a list of prompts";
  const made: Stored[] = [];
  for (const raw of body) {
    if (!isRec(raw)) return "a prompt is an object";
    const save = saveOf(raw);
    if (
      !save ||
      typeof raw.name !== "string" ||
      typeof raw.title !== "string" ||
      typeof raw.body !== "string"
    ) {
      return "a prompt has name, title and body";
    }
    const refused = validateSave(save);
    if (refused) return `a prompt is refused: ${refused.code} at ${refused.at}`;
    const id = newId(
      (x) => shelf.own.has(x) || shelf.others.has(x) || made.some((m) => m.id === x),
    );
    shelf.revision += 1;
    made.push({ id, save, revision: shelf.revision, updatedAt: now.toISOString() });
  }
  for (const m of made) shelf.others.set(m.id, m);
  return undefined;
}
