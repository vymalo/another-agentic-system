import {
  ANSWER_ACTION,
  type ChoicesSpec,
  QUESTION_TYPES,
  readChoices,
} from "@/features/chat/lib/a2ui/choices";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";

/*
 * The pure part of using a prompt from the message box (docs/api/prompts-v1.md, "The recipe"): what
 * opens the `/` menu, which prompts it offers, the Choices form a prompt's variables make, and the
 * words a refused `renderPrompt` is said in. Nothing here sends anything: the text goes into the
 * composer for the person to edit.
 */

/** A `/name` typed as the whole message, from its first character. */
export type PromptQuery = { query: string };

/**
 * The trigger of the `/` menu: the text begins with `/` and holds no white space, so the message is
 * one `/name` (`/` alone lists everything). A slash anywhere else, or a message with a space, is a
 * message and not a command.
 */
export function promptQueryOf(text: string): PromptQuery | null {
  if (!text.startsWith("/")) return null;
  const query = text.slice(1);
  return /\s/.test(query) ? null : { query };
}

const fold = (text: string): string =>
  text
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase();

const words = (text: string): string[] => fold(text).split(/[\s\-_/]+/);

/** Where a prompt matches a query: the name first, then a word of it, then the title. */
function rankOf(prompt: Pick<ApiPromptSummary, "name" | "title">, query: string): number {
  if (query === "") return 0;
  const name = fold(prompt.name);
  if (name === query) return 0;
  if (name.startsWith(query) || words(prompt.name).some((w) => w.startsWith(query))) return 1;
  if (name.includes(query)) return 2;
  if (words(prompt.title).some((w) => w.startsWith(query))) return 3;
  if (fold(prompt.title).includes(query)) return 4;
  return -1;
}

/**
 * The prompts the `/` menu offers for what follows the slash, best first: an exact name, then a
 * name that starts with it or a word of it, then a name that holds it, then the title. Ties are by
 * name and id, as the contract tells a client to sort (the server orders by id). Accents are folded,
 * so `/cafe` finds `/café`.
 */
export function matchingPrompts<T extends Pick<ApiPromptSummary, "id" | "name" | "title">>(
  prompts: readonly T[],
  query: string,
): T[] {
  const q = fold(query.trim());
  return prompts
    .map((prompt) => ({ prompt, rank: rankOf(prompt, q) }))
    .filter((r) => r.rank >= 0)
    .sort(
      (a, b) =>
        a.rank - b.rank ||
        (a.prompt.name < b.prompt.name ? -1 : a.prompt.name > b.prompt.name ? 1 : 0) ||
        (a.prompt.id < b.prompt.id ? -1 : a.prompt.id > b.prompt.id ? 1 : 0),
    )
    .map((r) => r.prompt);
}

/** The kinds this client draws; `images` needs a thread's files, which a prompt has not. */
const DRAWABLE = new Set<string>(QUESTION_TYPES.filter((type) => type !== "images"));

/**
 * The Choices form of a prompt's variables, or undefined when this client cannot draw one of them
 * (the contract: a client that cannot draw a kind does not offer the prompt). A prompt with no
 * variables is an empty form, which is filled with `answers: []`.
 */
export function specOf(prompt: Pick<ApiPrompt, "variables">): ChoicesSpec | undefined {
  for (const variable of prompt.variables) {
    const type = (variable.type ?? "single") as string;
    if (!DRAWABLE.has(type)) return undefined;
  }
  if (prompt.variables.length === 0) {
    return { componentId: "", questions: [], actionName: ANSWER_ACTION };
  }
  return readChoices({ questions: prompt.variables } as Record<string, unknown>);
}

/** The members of a `renderPrompt` problem a fill reads. */
export type FillProblem = {
  status: number;
  code?: string;
  /** The variable's id, or `"*"` for the answer as a whole. */
  question?: string;
  /** One of the contract's closed list of rules. */
  rule?: string;
};

/** A `renderPrompt` problem, from the error body and the status. */
export function fillProblemOf(error: unknown, status: number): FillProblem {
  const e = (error ?? {}) as { code?: unknown; question?: unknown; rule?: unknown };
  return {
    status,
    ...(typeof e.code === "string" ? { code: e.code } : {}),
    ...(typeof e.question === "string" ? { question: e.question } : {}),
    ...(typeof e.rule === "string" ? { rule: e.rule } : {}),
  };
}

export type FillFailure = {
  message: string;
  /** The question the server refused, when it named one. */
  question?: string;
  /** The prompt changed since it was read: offer "Load the current version". */
  stale?: boolean;
  /** The prompt is gone (or no longer shared): the list is read again. */
  gone?: boolean;
};

/** What a rule of the contract says, in words for a person. */
const RULES: Record<string, string> = {
  unknown: "it does not have the shape of an answer",
  unexpected: "it carries something the question does not take",
  required: "it is required",
  too_many: "it has too many choices",
  too_few: "it has too few choices",
  not_offered: "it is not one of the choices",
  repeated: "it names a choice twice",
  out_of_order: "its choices are not in the order of the options",
  too_long: "it is too long",
  grammar: "it is not in the form the question takes",
  below_min: "it is below the least",
  above_max: "it is above the most",
  off_step: "it is not one of the steps",
  not_a_permutation: "it does not name every option once",
};

const STALE =
  "This prompt was changed elsewhere since you opened it. Your answers are still here: press Load the current version to fill it as it is now.";

/**
 * What a refused `renderPrompt` says, in the words of its `code`, `question` and `rule`
 * (docs/api/prompts-v1.md, "Errors, and which wins"). The server never quotes a value, so nothing
 * here can show one that was not typed in the form.
 */
export function fillFailureOf(
  problem: FillProblem,
  spec: ChoicesSpec | undefined,
  maxRenderedBytes: number,
): FillFailure {
  const { status, code, question, rule } = problem;
  switch (code) {
    case "stale_revision":
      return { message: STALE, stale: true };
    case "invalid_answer": {
      if (question === "*" || question === undefined) {
        const extra = rule === "unexpected" ? " They carry something a prompt does not take." : "";
        return { message: `The answers do not fit this prompt's questions.${extra}` };
      }
      const asked = spec?.questions.find((q) => q.id === question);
      const name = asked ? `“${asked.question}”` : `“${question}”`;
      const why = (rule && RULES[rule]) ?? "the server did not say which rule failed";
      return {
        message: `The answer to ${name} is not accepted: ${why}.`,
        question,
      };
    }
    case "rendered_too_long":
      return {
        message: `The filled text would be over ${Math.round(maxRenderedBytes / 1024)} KiB. Shorten an answer.`,
      };
    default:
  }
  if (status === 404) {
    return {
      message: "This prompt no longer exists, or is no longer shared with you.",
      gone: true,
    };
  }
  if (status === 401) return { message: "Your session has ended. Sign in again." };
  if (status === 403) return { message: "Your roles do not allow this." };
  if (status === 429) return { message: "Too many requests. Try again in a moment." };
  if (status === 400 || status === 415) {
    return {
      message: "The app sent something the server could not read. Reload the page and try again.",
    };
  }
  if (status === 0 || status >= 500) {
    return { message: "The prompt could not be filled just now. Try again in a moment." };
  }
  return { message: "The prompt could not be filled: the server did not accept the answers." };
}
