/*
 * The model of the prompt form's questions (docs/api/prompts-v1.md "The variables"): the questions of a prompt as the
 * form edits them, and back. **Editing is lossless.** The page edits seven kinds of question (short text, long text, one
 * choice, several choices, yes or no, number, date). A question of any other kind is kept whole and written back as the
 * same object, and a question of an editable kind keeps every member the page does not show (a choice's `default`, an
 * option's `description` or `icon`, a member a later catalog adds): each member is written only when the form's value
 * differs from what was read, so a question nobody touched comes back byte for byte.
 */

export type Raw = Record<string, unknown>;

export type EditKind = "single" | "multiple" | "text" | "longText" | "confirm" | "number" | "date";

export const EDIT_KINDS: readonly EditKind[] = [
  "text",
  "longText",
  "single",
  "multiple",
  "confirm",
  "number",
  "date",
];

export const KIND_LABEL: Record<EditKind, string> = {
  text: "Short text",
  longText: "Long text",
  single: "One choice",
  multiple: "Several choices",
  confirm: "Yes or no",
  number: "Number",
  date: "Date",
};

/** An option as the form edits it. `raw` is the object that was read (its members the page does not show stay); null for a new one. */
export type Option = { value: string; label: string; raw: Raw | null };

export type Fields = {
  id: string;
  question: string;
  help: string;
  required: boolean;
  kind: EditKind;
  options: Option[];
  allowOther: boolean;
  yesLabel: string;
  noLabel: string;
  minValue: string;
  maxValue: string;
  step: string;
  unit: string;
  minDate: string;
  maxDate: string;
  /** What a short or long text starts with (`default`). */
  start: string;
};

export type EditableQuestion = {
  key: string;
  kept: false;
  /** The object that was read, or null for a question the person added. */
  raw: Raw | null;
  /** The fields as they were read: a member is written only when `fields` differs from it. */
  was: Fields;
  fields: Fields;
  /** The person has typed the id: it no longer follows the question's words. */
  idTouched: boolean;
};

/** A question the page cannot edit: written back as the very object that was read. */
export type KeptQuestion = {
  key: string;
  kept: true;
  raw: unknown;
  id: string;
  question: string;
  /** What it is, in a sentence ("a scale question", "an image question"). */
  phrase: string;
};

export type FormQuestion = EditableQuestion | KeptQuestion;

const COMMON = ["id", "question", "help", "required", "type"] as const;

/** The members each kind's card shows or decides (beside `COMMON`); any other member of the object is kept and not shown. */
const SHOWN: Record<EditKind, readonly string[]> = {
  single: ["options", "allowOther", "multiple"],
  multiple: ["options", "allowOther", "multiple"],
  text: ["default"],
  longText: ["default"],
  confirm: ["yesLabel", "noLabel"],
  number: ["minValue", "maxValue", "step", "unit"],
  date: ["minDate", "maxDate"],
};

const KEPT_NAMES: Record<string, string> = {
  select: "long-list",
  scale: "scale",
  rank: "ranking",
  images: "image",
};

const UNKNOWN_KIND = "a kind of question this page does not know";
const phraseOf = (type: unknown): string => {
  const name = typeof type === "string" ? KEPT_NAMES[type] : undefined;
  if (name === undefined) return UNKNOWN_KIND;
  return `${/^[aeiou]/.test(name) ? "an" : "a"} ${name} question`;
};

const isRec = (v: unknown): v is Raw => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const isEditKind = (v: unknown): v is EditKind =>
  typeof v === "string" && (EDIT_KINDS as readonly string[]).includes(v);
export const takesOptions = (kind: EditKind): boolean => kind === "single" || kind === "multiple";

let counter = 0;
const newKey = (): string => `q${++counter}`;

const blankFields = (): Fields => ({
  id: "",
  question: "",
  help: "",
  required: true,
  kind: "single",
  options: [],
  allowOther: false,
  yesLabel: "",
  noLabel: "",
  minValue: "",
  maxValue: "",
  step: "",
  unit: "",
  minDate: "",
  maxDate: "",
  start: "",
});

const cloneFields = (f: Fields): Fields => ({ ...f, options: f.options.map((o) => ({ ...o })) });

function readFields(raw: Raw, kind: EditKind): Fields {
  return {
    id: str(raw.id),
    question: str(raw.question),
    help: str(raw.help),
    required: raw.required !== false,
    kind,
    options: (Array.isArray(raw.options) ? raw.options : []).map((o) => {
      const rec = isRec(o) ? o : {};
      return { value: str(rec.value), label: str(rec.label), raw: isRec(o) ? o : {} };
    }),
    allowOther: raw.allowOther === true,
    yesLabel: str(raw.yesLabel),
    noLabel: str(raw.noLabel),
    minValue: str(raw.minValue),
    maxValue: str(raw.maxValue),
    step: str(raw.step),
    unit: str(raw.unit),
    minDate: str(raw.minDate),
    maxDate: str(raw.maxDate),
    start: str(raw.default),
  };
}

/** The questions of a prompt as the form edits them. Nothing is dropped: what cannot be edited is kept whole. */
export function toForm(variables: readonly unknown[]): FormQuestion[] {
  return variables.map((v, i): FormQuestion => {
    if (isRec(v)) {
      // the older spelling of several choices: no type and `multiple: true` (docs/api/ui-catalog-v1.md)
      const type = v.type === undefined ? (v.multiple === true ? "multiple" : "single") : v.type;
      if (isEditKind(type)) {
        const fields = readFields(v, type);
        return {
          key: newKey(),
          kept: false,
          raw: v,
          was: cloneFields(fields),
          fields,
          idTouched: true,
        };
      }
      return {
        key: newKey(),
        kept: true,
        raw: v,
        id: str(v.id),
        question: str(v.question),
        phrase: phraseOf(type),
      };
    }
    return {
      key: newKey(),
      kept: true,
      raw: v,
      id: "",
      question: `variable ${i + 1}`,
      phrase: UNKNOWN_KIND,
    };
  });
}

/** A question the person adds: one choice, required, two empty options (the options of a choice are 2 to 12). */
export function blankQuestion(_existing: readonly FormQuestion[]): EditableQuestion {
  const fields = blankFields();
  fields.options = [
    { value: "", label: "", raw: null },
    { value: "", label: "", raw: null },
  ];
  return {
    key: newKey(),
    kept: false,
    raw: null,
    was: cloneFields(fields),
    fields,
    idTouched: false,
  };
}

/** Whether a question holds members its card does not show (they are kept when it is saved). */
export function hasHiddenMembers(q: EditableQuestion): boolean {
  return q.fields.kind === q.was.kind && holdsHidden(q);
}

/** Whether the person has changed the kind of a question that holds members its card does not show: they will be removed. */
export function dropsHiddenMembers(q: EditableQuestion): boolean {
  return q.fields.kind !== q.was.kind && holdsHidden(q);
}

function holdsHidden(q: EditableQuestion): boolean {
  if (q.raw === null) return false;
  const shown = new Set<string>([...COMMON, ...SHOWN[q.was.kind]]);
  if (Object.keys(q.raw).some((k) => !shown.has(k))) return true;
  return q.raw.options !== undefined && Array.isArray(q.raw.options)
    ? q.raw.options.some(
        (o) => !isRec(o) || Object.keys(o).some((k) => k !== "value" && k !== "label"),
      )
    : false;
}

const same = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);
const optionsOf = (o: readonly Option[]) => o.map((x) => [x.value, x.label]);

const pick = (raw: Raw, keys: readonly string[]): Raw => {
  const out: Raw = {};
  for (const k of keys) if (k in raw) out[k] = raw[k];
  return out;
};

/** Lower-case ASCII words joined by hyphens, accents folded. */
const slug = (text: string): string =>
  text
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");

const unique = (base: string, taken: readonly string[]): string => {
  if (!taken.includes(base)) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base}-${n}`;
    if (!taken.includes(candidate)) return candidate;
  }
};

/** The value of a new option, made from its label (`[A-Za-z0-9_.:-]`, 1 to 64), unique among `taken`. */
export function valueFromLabel(label: string, taken: readonly string[]): string {
  return unique(slug(label).slice(0, 40).replace(/-+$/, "") || "option", taken);
}

/** The name of a new question in the text, made from the first words of its question, unique among `taken`. */
export function idFromWords(words: string, taken: readonly string[], n = taken.length + 1): string {
  const first = slug(words.trim().split(/\s+/).slice(0, 3).join(" "))
    .slice(0, 32)
    .replace(/-+$/, "");
  return unique(first || `answer-${n}`, taken);
}

/** What the questions of the form come to, ready to send. A kept question is the very object that was read. */
export function fromForm(questions: readonly FormQuestion[]): unknown[] {
  return questions.map((q) => (q.kept ? q.raw : write(q)));
}

function write(q: EditableQuestion): Raw {
  const f = q.fields;
  const w = q.was;
  const fresh = q.raw === null;
  const moved = !fresh && f.kind !== w.kind;
  const out: Raw = fresh
    ? {}
    : moved
      ? pick(q.raw as Raw, ["id", "question", "help", "required"])
      : { ...(q.raw as Raw) };
  // a member is written only when it differs from what was read (always, for a new question or a changed kind)
  const put = (key: string, now: string, before: string) => {
    if (!fresh && !moved && now === before) return;
    const text = now.trim();
    if (text === "") delete out[key];
    else out[key] = text;
  };
  put("id", f.id, w.id);
  put("question", f.question, w.question);
  put("help", f.help, w.help);
  if (fresh || moved || f.required !== w.required) {
    if (f.required) delete out.required;
    else out.required = false;
  }
  if (fresh || moved) out.type = f.kind;

  if (takesOptions(f.kind)) {
    if (fresh || moved || !same(optionsOf(f.options), optionsOf(w.options))) {
      // a new option's value is made from its label when it is saved, unique among the values already in use
      const taken = f.options.filter((o) => o.raw !== null).map((o) => o.value);
      out.options = f.options.map((o) => {
        const value = o.raw === null ? valueFromLabel(o.label, taken) : o.value;
        if (o.raw === null) taken.push(value);
        // an option nobody edited is the object that was read
        if (o.raw !== null && o.label === str(o.raw.label)) return o.raw;
        return { ...(o.raw ?? {}), value, label: o.label.trim() };
      });
    }
    if (fresh || moved || f.allowOther !== w.allowOther) {
      if (f.allowOther) out.allowOther = true;
      else delete out.allowOther;
    }
  }
  if (f.kind === "text" || f.kind === "longText") put("default", f.start, w.start);
  if (f.kind === "confirm") {
    put("yesLabel", f.yesLabel, w.yesLabel);
    put("noLabel", f.noLabel, w.noLabel);
  }
  if (f.kind === "number") {
    put("minValue", f.minValue, w.minValue);
    put("maxValue", f.maxValue, w.maxValue);
    put("step", f.step, w.step);
    put("unit", f.unit, w.unit);
  }
  if (f.kind === "date") {
    put("minDate", f.minDate, w.minDate);
    put("maxDate", f.maxDate, w.maxDate);
  }
  return out;
}
