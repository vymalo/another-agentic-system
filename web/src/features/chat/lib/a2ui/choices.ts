/**
 * The `Choices` component of the UI catalog (docs/api/ui-catalog-v1.md, version 2): up to 8
 * questions asked at once, each with 2 to 8 options, answered with one action.
 *
 * Pure: what a Choices says, what an answer is, and how an answer reads back to the person. The
 * JSON Schema in `catalog/catalog.json` is what makes an instance valid; this module reads one
 * that passed it (and, for the answer bubble, defensively, because the operations it reads come
 * from the transcript and nothing has checked them for this purpose).
 */

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The name an answer's action has when the component names none. */
export const ANSWER_ACTION = "answer";
/** The most characters of a free answer (the contract's `other`). */
export const MAX_OTHER = 500;

export type ChoiceOption = { value: string; label: string; description?: string };

export type ChoiceQuestion = {
  id: string;
  question: string;
  options: ChoiceOption[];
  multiple: boolean;
  allowOther: boolean;
  /** A question is required unless the component says `required: false`. */
  required: boolean;
};

export type ChoicesSpec = {
  /** The component's id: the `sourceComponentId` of the action. */
  componentId: string;
  title?: string;
  questions: ChoiceQuestion[];
  submitLabel?: string;
  /** `action.event.name`, else {@link ANSWER_ACTION}. */
  actionName: string;
};

function readOption(v: unknown): ChoiceOption | undefined {
  if (!isRecord(v)) return undefined;
  const value = str(v.value);
  const label = str(v.label);
  if (value === undefined || label === undefined) return undefined;
  const description = str(v.description);
  return { value, label, ...(description ? { description } : {}) };
}

function readQuestion(v: unknown): ChoiceQuestion | undefined {
  if (!isRecord(v)) return undefined;
  const id = str(v.id);
  const question = str(v.question);
  if (id === undefined || question === undefined || !Array.isArray(v.options)) return undefined;
  const options = v.options.map(readOption);
  if (options.some((o) => o === undefined)) return undefined;
  return {
    id,
    question,
    options: options as ChoiceOption[],
    multiple: v.multiple === true,
    allowOther: v.allowOther === true,
    required: v.required !== false,
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
  const submitLabel = str(component.submitLabel);
  return {
    componentId: str(component.componentId) ?? str(component.id) ?? "",
    ...(title ? { title } : {}),
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

// ---- answering ----------------------------------------------------------------------------

/** What the person has chosen in one question so far. */
export type QuestionState = {
  /** Option values, in the order of the options. */
  values: string[];
  /** The "Other" choice is on (the radio is selected, or the box is checked). */
  otherOn: boolean;
  other: string;
};
/** By question id: a Map, because an id may be `__proto__` or `constructor` (the pattern allows them). */
export type AnswerState = ReadonlyMap<string, QuestionState>;

export const EMPTY: QuestionState = { values: [], otherOn: false, other: "" };

/** What the person typed as their own answer, when "Other" is on and it says something. */
const otherOf = (q: ChoiceQuestion, s: QuestionState): string | undefined => {
  const text = s.other.trim();
  return q.allowOther && s.otherOn && text !== "" ? text.slice(0, MAX_OTHER) : undefined;
};

/** A question counts as answered when an option is chosen, or "Other" is on and not empty. */
export const isAnswered = (q: ChoiceQuestion, s: QuestionState = EMPTY): boolean =>
  s.values.length > 0 || otherOf(q, s) !== undefined;

/** How many required questions have no answer yet. */
export const missing = (spec: ChoicesSpec, state: AnswerState): number =>
  spec.questions.filter((q) => q.required && !isAnswered(q, state.get(q.id))).length;

/** One question's answer as the contract has it: `{id, values, other?}`. */
export type Answer = { id: string; values: string[]; other?: string };

/** The answers of the action's context, in question order, one per question (empty when skipped). */
export function buildAnswers(spec: ChoicesSpec, state: AnswerState): Answer[] {
  return spec.questions.map((q) => {
    const s = state.get(q.id) ?? EMPTY;
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

/** The context an answer's action carries (`context.answers`, ui-catalog-v1.md "Choices answers"). */
export const answerContext = (spec: ChoicesSpec, state: AnswerState): { answers: Answer[] } => ({
  answers: buildAnswers(spec, state),
});

// ---- reading an answer back ---------------------------------------------------------------

/** Answers a person could give: at most this many are read back (the component has 8). */
const MAX_ANSWERS = 50;

/**
 * The `answers` of an action's context when it has the shape of an answer, else null: a list of
 * `{id, values: [text…], other?: text}`. Anything else is not read as an answer.
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
    out.push({
      id,
      values: a.values as string[],
      ...(a.other !== undefined ? { other: a.other } : {}),
    });
  }
  return out;
}

/** An answer as the person reads it: the question, and what they chose. */
export type AnswerLine = {
  id: string;
  /** The question's words, or its id when the surface is not in the transcript. */
  question: string;
  /** The labels chosen, else the raw values. */
  chosen: string[];
  other?: string;
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
    };
  });
}

/**
 * The questions of the Choices component `componentId` of surface `surfaceId`, read from the raw
 * operations of the surface (the last definition of a component wins, a later `createSurface`
 * starts the surface over). With no `componentId`, the first Choices of the surface. Undefined
 * when the surface has no such component.
 */
export function questionsOf(
  operations: unknown,
  surfaceId: string,
  componentId?: string,
): ChoiceQuestion[] | undefined {
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
    return readChoices(c)?.questions;
  }
  return undefined;
}
