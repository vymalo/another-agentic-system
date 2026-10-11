/**
 * The `Choices` component of the UI catalog (docs/api/ui-catalog-v1.md, version 5): a form of up to
 * 8 questions asked at once and answered with one action. A form has a title, a description of why
 * the agent asks and an optional free box; a question has a help line and a type (radio buttons,
 * check boxes, a searchable list, a deck of pictures, or a typed answer); an option has a label, a
 * description, a picture, an icon and a "recommended" mark. Every payload of version 4 is a payload
 * of version 5 and reads and answers as it did.
 *
 * Pure: what a Choices says, what an answer is, and how an answer reads back to the person. The
 * JSON Schema in `catalog/catalog.json` is what makes an instance valid; `limitsIn` is what it
 * cannot say (the counts that depend on the type, defaults, a picture for every tile of a deck).
 * This module reads one that passed it (and, for the answer bubble, defensively, because the
 * operations it reads come from the transcript and nothing has checked them for this purpose).
 */

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The name an answer's action has when the component names none. */
export const ANSWER_ACTION = "answer";
/** The most characters of a free answer (the contract's `other`). */
export const MAX_OTHER = 500;
/** The most characters of a `text` answer, of a `longText` answer and of the form's `note`. */
export const MAX_TEXT = 500;
export const MAX_LONG_TEXT = 2000;
export const MAX_NOTE = 2000;
/** The limits of a form: questions, options of a radio or checkbox question, of a list, of a deck. */
export const MAX_QUESTIONS = 8;
export const MAX_OPTIONS = 12;
export const MAX_SELECT_OPTIONS = 50;
export const MAX_DECK_IMAGES = 16;
/** The fewest options a question that offers any has. */
export const MIN_OPTIONS = 2;
/** What the free box is called when the agent gives it no label. */
export const FREE_INPUT_LABEL = "Anything else you want to add?";

export const QUESTION_TYPES = [
  "single",
  "multiple",
  "select",
  "images",
  "text",
  "longText",
] as const;
export type QuestionType = (typeof QUESTION_TYPES)[number];

/** A `text` or `longText` question: the person types the answer, there are no options. */
export const isTyping = (type: QuestionType): boolean => type === "text" || type === "longText";

/** A picture of a file of the thread, by its SHA-256 (never a URL). */
export type OptionImage = { sha256: string; alt: string };

export type ChoiceOption = {
  value: string;
  label: string;
  description?: string;
  image?: OptionImage;
  /** One of the steps/v1 icon ids; the catalog's schema has checked it. */
  icon?: string;
  /** Drawn with a "Recommended" mark. */
  recommended?: boolean;
};

export type ChoiceQuestion = {
  id: string;
  question: string;
  /** A line under the question: what the options mean, or what the answer is for. */
  help?: string;
  /** What the question says it is, else `multiple` or `single` by its `multiple` flag. */
  type: QuestionType;
  options: ChoiceOption[];
  /** More than one option may be chosen: type `multiple`, or `multiple: true` on a list or a deck. */
  multiple: boolean;
  allowOther: boolean;
  /** A question is required unless the component says `required: false`. */
  required: boolean;
  /** The option values chosen before the person touches anything. */
  defaults: string[];
  /** The text a `text` or `longText` question starts with. */
  defaultText?: string;
  /** The fewest and most choices when several may be chosen. */
  min?: number;
  max?: number;
  /** What the component said, kept to find the contradictions the schema cannot. */
  declared: { type?: QuestionType; multiple?: boolean };
};

export type FreeInput = { label: string; placeholder?: string };

export type ChoicesSpec = {
  /** The component's id: the `sourceComponentId` of the action. */
  componentId: string;
  title?: string;
  /** Why the agent asks, in plain text. */
  description?: string;
  /** An "anything else" box; its text is the answer's `note`. */
  freeInput?: FreeInput;
  questions: ChoiceQuestion[];
  submitLabel?: string;
  /** `action.event.name`, else {@link ANSWER_ACTION}. */
  actionName: string;
};

function readImage(v: unknown): OptionImage | undefined {
  if (!isRecord(v)) return undefined;
  const sha256 = str(v.sha256);
  const alt = str(v.alt);
  return sha256 === undefined || alt === undefined ? undefined : { sha256, alt };
}

function readOption(v: unknown): ChoiceOption | undefined {
  if (!isRecord(v)) return undefined;
  const value = str(v.value);
  const label = str(v.label);
  if (value === undefined || label === undefined) return undefined;
  const description = str(v.description);
  const image = readImage(v.image);
  const icon = str(v.icon);
  return {
    value,
    label,
    ...(description ? { description } : {}),
    ...(image ? { image } : {}),
    ...(icon ? { icon } : {}),
    ...(v.recommended === true ? { recommended: true } : {}),
  };
}

const typeOf = (v: unknown): QuestionType | undefined => QUESTION_TYPES.find((t) => t === v);
const count = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isInteger(v) ? v : undefined;

function readQuestion(v: unknown): ChoiceQuestion | undefined {
  if (!isRecord(v)) return undefined;
  const id = str(v.id);
  const question = str(v.question);
  if (id === undefined || question === undefined) return undefined;
  // a question that is typed has no options; one that has none is for `limitsIn` to refuse
  if (v.options !== undefined && !Array.isArray(v.options)) return undefined;
  const options = (v.options ?? []).map(readOption) as (ChoiceOption | undefined)[];
  if (options.some((o) => o === undefined)) return undefined;

  const typeGiven = typeOf(v.type);
  const multipleGiven = typeof v.multiple === "boolean" ? v.multiple : undefined;
  const type = typeGiven ?? (multipleGiven === true ? "multiple" : "single");
  // `multiple: true` is the old spelling of type `multiple`; it also makes a list or a deck pick many
  const multiple =
    type === "multiple"
      ? true
      : type === "select" || type === "images"
        ? multipleGiven === true
        : false;
  const help = str(v.help);
  const given = v.default;
  const defaults = Array.isArray(given)
    ? given.filter((d): d is string => typeof d === "string")
    : typeof given === "string"
      ? [given]
      : [];
  const min = count(v.min);
  const max = count(v.max);
  return {
    id,
    question,
    ...(help ? { help } : {}),
    type,
    options: options as ChoiceOption[],
    multiple,
    allowOther: v.allowOther === true,
    required: v.required !== false,
    defaults: isTyping(type) ? [] : defaults,
    ...(isTyping(type) && typeof given === "string" && given !== "" ? { defaultText: given } : {}),
    ...(min !== undefined ? { min } : {}),
    ...(max !== undefined ? { max } : {}),
    declared: {
      ...(typeGiven ? { type: typeGiven } : {}),
      ...(multipleGiven !== undefined ? { multiple: multipleGiven } : {}),
    },
  };
}

function readFreeInput(v: unknown): FreeInput | undefined {
  if (!isRecord(v)) return undefined;
  const label = str(v.label);
  const placeholder = str(v.placeholder);
  return {
    label: label ? label : FREE_INPUT_LABEL,
    ...(placeholder ? { placeholder } : {}),
  };
}

/**
 * The Choices a component object says, or undefined when it is not one. `componentId` is passed
 * by the validator (the converter keeps a component's id out of its props); a raw component brings
 * its own `id`.
 */
export function readChoices(component: Rec): ChoicesSpec | undefined {
  if (!Array.isArray(component.questions)) return undefined;
  const questions = component.questions.map(readQuestion);
  if (questions.length === 0 || questions.some((q) => q === undefined)) return undefined;
  const event = isRecord(component.action) ? component.action.event : undefined;
  const name = isRecord(event) ? str(event.name) : undefined;
  const title = str(component.title);
  const description = str(component.description);
  const freeInput = readFreeInput(component.freeInput);
  const submitLabel = str(component.submitLabel);
  return {
    componentId: str(component.componentId) ?? str(component.id) ?? "",
    ...(title ? { title } : {}),
    ...(description ? { description } : {}),
    ...(freeInput ? { freeInput } : {}),
    questions: questions as ChoiceQuestion[],
    ...(submitLabel ? { submitLabel } : {}),
    actionName: name || ANSWER_ACTION,
  };
}

/**
 * What JSON Schema cannot say: question ids are unique in a Choices, option values are unique
 * within a question. The reason is for a refusal; `undefined` means both hold.
 */
export function duplicateIn(spec: ChoicesSpec): string | undefined {
  const ids = new Set<string>();
  for (const q of spec.questions) {
    if (ids.has(q.id)) return `the question id ${JSON.stringify(q.id)} is used twice`;
    ids.add(q.id);
    const values = new Set<string>();
    for (const o of q.options) {
      if (values.has(o.value)) {
        return `the option value ${JSON.stringify(o.value)} is used twice in the question ${JSON.stringify(q.id)}`;
      }
      values.add(o.value);
    }
  }
  return undefined;
}

const OPTIONS_OF: Record<Exclude<QuestionType, "text" | "longText">, [number, string]> = {
  single: [MAX_OPTIONS, 'a radio question takes 2 to 12 options (use type "select" for up to 50)'],
  multiple: [
    MAX_OPTIONS,
    'a checkbox question takes 2 to 12 options (use type "select" with multiple: true for up to 50)',
  ],
  select: [MAX_SELECT_OPTIONS, 'a "select" question takes 2 to 50 options'],
  images: [MAX_DECK_IMAGES, 'an "images" question takes 2 to 16 options, each with a picture'],
};

/**
 * What JSON Schema cannot say about the questions, as a refusal's reason (`undefined`: all hold):
 * the options a type takes (2 to 12, 50 for a list, 16 pictures for a deck, none for a typed
 * answer), a picture for every tile of a deck and for no other option, a type that contradicts
 * `multiple`, a default that is not one of the options, and `min` and `max`, which only count
 * choices when several may be chosen.
 */
export function limitsIn(spec: ChoicesSpec): string | undefined {
  if (spec.questions.length > MAX_QUESTIONS) {
    return `a form has at most ${MAX_QUESTIONS} questions, this has ${spec.questions.length}`;
  }
  for (const q of spec.questions) {
    const at = `the question ${JSON.stringify(q.id)}`;
    const { type: declaredType, multiple: declaredMultiple } = q.declared;
    if (declaredType === "single" && declaredMultiple === true) {
      return `${at} is type "single" but has multiple: true`;
    }
    if (declaredType === "multiple" && declaredMultiple === false) {
      return `${at} is type "multiple" but has multiple: false`;
    }
    if (isTyping(q.type)) {
      const limit = q.type === "text" ? MAX_TEXT : MAX_LONG_TEXT;
      if (q.options.length > 0) return `${at} is type "${q.type}": a typed answer takes no options`;
      if (declaredMultiple === true)
        return `${at} is type "${q.type}": it cannot have multiple: true`;
      if (q.allowOther) return `${at} is type "${q.type}": it cannot have allowOther`;
      if (q.min !== undefined || q.max !== undefined) {
        return `${at} is type "${q.type}": min and max count choices, not typed words`;
      }
      if (q.defaultText !== undefined && q.defaultText.length > limit) {
        return `${at} has a default of ${q.defaultText.length} characters; a "${q.type}" answer is at most ${limit}`;
      }
      continue;
    }
    const [most, hint] = OPTIONS_OF[q.type as keyof typeof OPTIONS_OF];
    if (q.options.length < MIN_OPTIONS || q.options.length > most) {
      return `${at} has ${q.options.length} options: ${hint}`;
    }
    for (const o of q.options) {
      if (q.type === "images" && !o.image) {
        return `the option ${JSON.stringify(o.value)} of ${at} has no image: every tile of a deck needs one`;
      }
      if (q.type !== "images" && o.image) {
        return `the option ${JSON.stringify(o.value)} of ${at} has an image, which only type "images" draws`;
      }
    }
    const values = new Set(q.options.map((o) => o.value));
    const unknown = q.defaults.find((d) => !values.has(d));
    if (unknown !== undefined) {
      return `the default ${JSON.stringify(unknown)} of ${at} is not one of its options`;
    }
    if (!q.multiple && q.defaults.length > 1) {
      return `${at} takes one choice, but has ${q.defaults.length} defaults`;
    }
    const cap = q.options.length + (q.allowOther ? 1 : 0);
    if (!q.multiple) {
      if (q.min !== undefined || q.max !== undefined) {
        return `${at} takes one choice: min and max apply when several may be chosen`;
      }
      continue;
    }
    if (q.max !== undefined && (q.max < 1 || q.max > cap)) {
      return `the max of ${at} is ${q.max}, but it offers ${cap} choices`;
    }
    if (q.min !== undefined && q.min > cap) {
      return `the min of ${at} is ${q.min}, but it offers ${cap} choices`;
    }
    if (q.min !== undefined && q.max !== undefined && q.min > q.max) {
      return `the min of ${at} (${q.min}) is above its max (${q.max})`;
    }
    if (q.max !== undefined && q.defaults.length > q.max) {
      return `${at} has ${q.defaults.length} defaults, above its max ${q.max}`;
    }
  }
  return undefined;
}

// ---- answering ----------------------------------------------------------------------------

/** What the person has chosen (or typed) in one question so far. */
export type QuestionState = {
  /** Option values, in the order of the options. */
  values: string[];
  /** The "Other" choice is on (the radio is selected, or the box is checked). */
  otherOn: boolean;
  other: string;
  /** The typed answer of a `text` or `longText` question. */
  text: string;
};
/** By question id: a Map, because an id may be `__proto__` or `constructor` (the pattern allows them). */
export type AnswerState = ReadonlyMap<string, QuestionState>;

export const EMPTY: QuestionState = { values: [], otherOn: false, other: "", text: "" };

/** The answers a form starts with: the defaults, which the person can change before sending. */
export function initialState(spec: ChoicesSpec): AnswerState {
  const state = new Map<string, QuestionState>();
  for (const q of spec.questions) {
    if (q.defaultText !== undefined) {
      state.set(q.id, { ...EMPTY, text: q.defaultText });
      continue;
    }
    const known = new Set(q.options.map((o) => o.value));
    const values = q.defaults.filter((d) => known.has(d)).slice(0, q.multiple ? undefined : 1);
    if (values.length > 0) state.set(q.id, { ...EMPTY, values });
  }
  return state;
}

/** What the person typed as their own answer, when "Other" is on and it says something. */
const otherOf = (q: ChoiceQuestion, s: QuestionState): string | undefined => {
  const text = s.other.trim();
  return q.allowOther && s.otherOn && text !== "" ? text.slice(0, MAX_OTHER) : undefined;
};

/** What a typed question holds: the words, trimmed and cut at the question's limit. */
const typedOf = (q: ChoiceQuestion, s: QuestionState): string | undefined => {
  const text = s.text.trim();
  return text === "" ? undefined : text.slice(0, q.type === "text" ? MAX_TEXT : MAX_LONG_TEXT);
};

/** How many choices a question has: the options chosen and an "Other" with words, or 1 for typed words. */
export const chosenCount = (q: ChoiceQuestion, s: QuestionState = EMPTY): number =>
  isTyping(q.type)
    ? typedOf(q, s) === undefined
      ? 0
      : 1
    : s.values.length + (otherOf(q, s) !== undefined ? 1 : 0);

/** A question counts as answered when an option is chosen, or "Other" is on and not empty, or words are typed. */
export const isAnswered = (q: ChoiceQuestion, s: QuestionState = EMPTY): boolean =>
  chosenCount(q, s) > 0;

/**
 * Whether the question lets the form go: a required one needs an answer (and `min` of them when
 * several may be chosen), an optional one needs none, but what is chosen must still fit `min` and
 * `max`.
 */
export function isSatisfied(q: ChoiceQuestion, s: QuestionState = EMPTY): boolean {
  const n = chosenCount(q, s);
  if (n === 0) return !q.required;
  if (q.min !== undefined && n < q.min) return false;
  return q.max === undefined || n <= q.max;
}

/** How many questions still hold the form back: required and unanswered, or short of `min`. */
export const missing = (spec: ChoicesSpec, state: AnswerState): number =>
  spec.questions.filter((q) => !isSatisfied(q, state.get(q.id))).length;

/** One question's answer as the contract has it: `{id, values, other?, text?}`. */
export type Answer = { id: string; values: string[]; other?: string; text?: string };

/** The answers of the action's context, in question order, one per question (empty when skipped). */
export function buildAnswers(spec: ChoicesSpec, state: AnswerState): Answer[] {
  return spec.questions.map((q) => {
    const s = state.get(q.id) ?? EMPTY;
    if (isTyping(q.type)) {
      const text = typedOf(q, s);
      return { id: q.id, values: [], ...(text !== undefined ? { text } : {}) };
    }
    const other = otherOf(q, s);
    const chosen = new Set(s.values);
    return {
      id: q.id,
      // in the order of the options, never of the clicks, and only values the question has
      values: q.options.filter((o) => chosen.has(o.value)).map((o) => o.value),
      ...(other !== undefined ? { other } : {}),
    };
  });
}

/** What the person wrote in the free box, trimmed and cut, or undefined when there is no such box or it is empty. */
export const noteOf = (spec: ChoicesSpec, note: string): string | undefined => {
  const text = note.trim();
  return spec.freeInput && text !== "" ? text.slice(0, MAX_NOTE) : undefined;
};

/** What an answer's action carries as `context`: the answers, and the free box's words as `note`. */
export type AnswerContext = { answers: Answer[]; note?: string };

/** The context an answer's action carries (`context.answers`, ui-catalog-v1.md "Choices answers"). */
export function answerContext(spec: ChoicesSpec, state: AnswerState, note = ""): AnswerContext {
  const text = noteOf(spec, note);
  return { answers: buildAnswers(spec, state), ...(text !== undefined ? { note: text } : {}) };
}

// ---- reading an answer back ---------------------------------------------------------------

/** Answers a person could give: at most this many are read back (the component has 8). */
const MAX_ANSWERS = 50;

/**
 * The `answers` of an action's context when it has the shape of an answer, else null: a list of
 * `{id, values: [text…], other?: text, text?: text}`. Anything else is not read as an answer.
 */
export function readAnswers(context: unknown): Answer[] | null {
  if (!isRecord(context) || !Array.isArray(context.answers)) return null;
  if (context.answers.length === 0 || context.answers.length > MAX_ANSWERS) return null;
  const out: Answer[] = [];
  for (const a of context.answers) {
    if (!isRecord(a)) return null;
    const id = str(a.id);
    if (id === undefined || !Array.isArray(a.values)) return null;
    if (!a.values.every((v) => typeof v === "string")) return null;
    if (a.other !== undefined && typeof a.other !== "string") return null;
    if (a.text !== undefined && typeof a.text !== "string") return null;
    out.push({
      id,
      values: a.values as string[],
      ...(a.other !== undefined ? { other: a.other } : {}),
      ...(a.text !== undefined ? { text: a.text } : {}),
    });
  }
  return out;
}

/** The free box's words of an answer's context, when it has any. */
export function readNote(context: unknown): string | undefined {
  const note = isRecord(context) ? str(context.note) : undefined;
  return note === undefined || note.trim() === "" ? undefined : note;
}

/** An answer as the person reads it: the question, and what they chose. */
export type AnswerLine = {
  id: string;
  /** The question's words, or its id when the surface is not in the transcript. */
  question: string;
  /** The labels chosen, else the raw values. */
  chosen: string[];
  other?: string;
  /** What they typed, for a `text` or `longText` question. */
  text?: string;
};

/**
 * The lines of "Your answers": each answer with its question and the option labels, resolved from
 * the surface the action names; a question or a value the surface does not have shows as it was
 * sent (the id, the raw value), never as nothing.
 */
export function answerLines(
  answers: readonly Answer[],
  questions: readonly ChoiceQuestion[] | undefined,
): AnswerLine[] {
  return answers.map((a) => {
    const q = questions?.find((x) => x.id === a.id);
    const label = (value: string) => q?.options.find((o) => o.value === value)?.label ?? value;
    return {
      id: a.id,
      question: q?.question ?? a.id,
      chosen: a.values.map(label),
      ...(a.other !== undefined ? { other: a.other } : {}),
      ...(a.text !== undefined ? { text: a.text } : {}),
    };
  });
}

/**
 * The Choices component `componentId` of surface `surfaceId`, read from the raw operations of the
 * surface (the last definition of a component wins, a later `createSurface` starts the surface
 * over). With no `componentId`, the first Choices of the surface. Undefined when the surface has no
 * such component.
 */
export function choicesOf(
  operations: unknown,
  surfaceId: string,
  componentId?: string,
): ChoicesSpec | undefined {
  if (!Array.isArray(operations)) return undefined;
  let components = new Map<string, Rec>();
  for (const op of operations) {
    if (!isRecord(op)) continue;
    const create = op.createSurface;
    if (isRecord(create) && create.surfaceId === surfaceId) components = new Map();
    const update = op.updateComponents;
    if (!isRecord(update) || update.surfaceId !== surfaceId || !Array.isArray(update.components)) {
      continue;
    }
    for (const c of update.components) {
      const id = isRecord(c) ? str(c.id) : undefined;
      if (isRecord(c) && id !== undefined) components.set(id, c);
    }
  }
  for (const [id, c] of components) {
    if (c.component !== "Choices") continue;
    if (componentId !== undefined && componentId !== "" && id !== componentId) continue;
    return readChoices(c);
  }
  return undefined;
}

/** The questions of {@link choicesOf}. */
export const questionsOf = (
  operations: unknown,
  surfaceId: string,
  componentId?: string,
): ChoiceQuestion[] | undefined => choicesOf(operations, surfaceId, componentId)?.questions;
