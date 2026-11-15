/*
 * What a refused or failed write of a prompt says, in words, and where on the form (docs/api/prompts-v1.md, "Errors, and which
 * wins"). The words come from the problem's `code`; its `at` says which member (`name`, `body`, `variables.<i>.<member>`,
 * `variables.<i>.options.<j>.<member>`) and `offset` where in the body (UTF-16 units, so it indexes a JavaScript string as it
 * is). The server never quotes a value in a problem, so nothing here can show one that was not typed on this page. A code this
 * file does not know is still answered truthfully, from the status.
 */

export type Target =
  | { field: "name" | "title" | "description" | "body" | "audience" | "questions" | "form" }
  | { field: "question"; index: number; path: string[] };

export type Problem = {
  status: number;
  code?: string | undefined;
  at?: string | undefined;
  offset?: number | undefined;
};

export type Context = {
  /** `getConfig.prompts.limit`. */
  limit: number;
  maxVariables: number;
  /** The name as sent, for "You already have a prompt named /x". */
  name: string;
  /** The body as sent: `undeclared_variable` reads the id at the offset. */
  body: string;
  /** The ids of the questions as sent, in order. */
  ids: readonly string[];
};

export type Failure = {
  message: string;
  target: Target;
  /** A range of the body to select (UTF-16 units), where the problem names one. */
  select?: { start: number; end: number };
  /** The outcome is not what the person asked for or is not known: the list must be read again. */
  reread: boolean;
  /** The prompt changed since it was opened: the form keeps the text and offers the current version. */
  stale?: boolean;
  /** 403 `forbidden`: the roles no longer let the person keep a share in place. */
  forbidden?: boolean;
};

const FORM: Target = { field: "form" };
const MEMBERS = ["name", "title", "description", "body", "audience"] as const;

/** Where a save was refused, from `at`; anything this does not understand is the form as a whole. */
export function parseAt(at: string | undefined): Target {
  if (at === undefined) return FORM;
  if ((MEMBERS as readonly string[]).includes(at)) return { field: at as (typeof MEMBERS)[number] };
  if (at === "variables") return { field: "questions" };
  const m = /^variables\.(0|[1-9][0-9]{0,2})((?:\.[A-Za-z0-9_]+)*)$/.exec(at);
  if (!m) return FORM;
  const path = (m[2] ?? "").split(".").filter((s) => s !== "");
  return { field: "question", index: Number(m[1]), path };
}

const MEMBER_WORDS: Record<string, string> = {
  id: "the name in the text",
  question: "the question text",
  help: "the help line",
  required: "“Required”",
  type: "the kind",
  options: "the options",
  allowOther: "“Let people write their own answer”",
  default: "the starting answer",
  yesLabel: "the Yes label",
  noLabel: "the No label",
  minValue: "the smallest value",
  maxValue: "the largest value",
  step: "the step",
  unit: "the unit",
  minDate: "the earliest date",
  maxDate: "the latest date",
};

/** `["options", "1", "label"]` as "option 2's label"; a member this page has no word for is named by itself. */
function memberWords(path: readonly string[]): string {
  if (path[0] === "options" && path[1] !== undefined && /^\d+$/.test(path[1])) {
    const n = Number(path[1]) + 1;
    return path[2] === undefined ? `option ${n}` : `option ${n}'s ${path[2]}`;
  }
  const head = path[0];
  if (head === undefined) return "";
  return MEMBER_WORDS[head] ?? `the setting “${head}”`;
}

const BLANK = /^\{\{([A-Za-z0-9_.:-]{1,64})\}\}/;

const STALE =
  "This prompt was changed elsewhere since you opened it. Your changes are still here: copy what you want to keep, then press Load the current version to start again from it.";

const LOST =
  "Your prompt could not be saved just now, and the change may or may not have been made. The list was read again: check it.";

export function failureOf(problem: Problem, ctx: Context): Failure {
  const { status, code, at, offset } = problem;
  const target = parseAt(at);
  const say = (
    message: string,
    extra: Omit<Failure, "message" | "target" | "reread"> & {
      reread?: boolean;
      target?: Target;
    } = {},
  ): Failure => {
    const { reread = false, target: t = target, ...rest } = extra;
    return { message, target: t, reread, ...rest };
  };
  switch (code) {
    case "bad_name":
      return say(
        "A name is 1 to 32 lowercase letters, digits or hyphens, and starts with a letter or a digit.",
        { target: { field: "name" } },
      );
    case "name_taken":
      return say(`You already have a prompt named /${ctx.name}. Choose another name.`, {
        target: { field: "name" },
      });
    case "bad_title":
      return say("A title is 1 to 120 characters on one line, with no space at either end.", {
        target: { field: "title" },
      });
    case "bad_description":
      return say("A description is at most 500 characters, with no control characters.", {
        target: { field: "description" },
      });
    case "bad_body":
      return say(
        "The text is empty, longer than 16 KB, or holds a character that cannot be saved.",
        {
          target: { field: "body" },
        },
      );
    case "bad_reference":
      return say(
        "These braces do not start a blank. Write {{name}} with no spaces, or \\{{ for literal braces.",
        {
          target: { field: "body" },
          ...(typeof offset === "number" ? { select: { start: offset, end: offset + 2 } } : {}),
        },
      );
    case "undeclared_variable": {
      const found = typeof offset === "number" ? BLANK.exec(ctx.body.slice(offset)) : null;
      if (!found) {
        return say(
          "A blank in the text is not one of the questions. Add a question with that name, or take it out of the text.",
          { target: { field: "body" } },
        );
      }
      const id = found[1] as string;
      return say(
        `{{${id}}} is not one of the questions. Add a question named ${id}, or take it out of the text.`,
        {
          target: { field: "body" },
          select: { start: offset as number, end: (offset as number) + found[0].length },
        },
      );
    }
    case "unused_variable": {
      const id = target.field === "question" ? ctx.ids[target.index] : undefined;
      return say(
        id
          ? `This question is not used in the text. Write {{${id}}} where its answer goes, or remove the question.`
          : "This question is not used in the text. Write its name in braces where its answer goes, or remove the question.",
      );
    }
    case "bad_variable": {
      if (target.field === "questions") {
        return say(`A prompt has at most ${ctx.maxVariables} questions.`);
      }
      if (target.field === "question") {
        const word = memberWords(target.path);
        const n = target.index + 1;
        return say(
          word === ""
            ? `Question ${n} cannot be saved as it is.`
            : `Question ${n}: ${word} cannot be saved as it is.`,
        );
      }
      return say("A question cannot be saved as it is.", { target: FORM });
    }
    case "bad_audience":
      return say(
        "A role is 1 to 64 characters with no spaces. Everyone can only be chosen alone. At most 32 roles.",
        { target: { field: "audience" } },
      );
    case "too_many_prompts":
      return say(
        `You have ${ctx.limit} prompts, the most one person can have. Delete one to add another.`,
        { target: FORM, reread: true },
      );
    case "stale_revision":
      return say(STALE, { target: FORM, reread: true, stale: true });
    case "forbidden":
      return say(
        "Your roles no longer let you share prompts, so a shared prompt cannot be changed. Stop sharing it first, then change it.",
        { target: FORM, forbidden: true },
      );
    case "no_access":
      return say("Your roles grant no access to this app.", { target: FORM });
    default:
  }
  if (status === 404) return say("This prompt no longer exists.", { target: FORM, reread: true });
  if (status === 400 || status === 415) {
    return say("The app sent something the server could not read. Reload the page and try again.", {
      target: FORM,
    });
  }
  if (status === 0 || status >= 500) return say(LOST, { target: FORM, reread: true });
  if (status === 401) return say(SIGNED_OUT, { target: FORM });
  if (status === 403) return say(NOT_ALLOWED, { target: FORM });
  if (status === 429) return say(TOO_MANY, { target: FORM });
  return say("Your prompt could not be saved: the server did not accept it.", { target: FORM });
}

const SIGNED_OUT = "Your session has ended. Sign in again.";
const NOT_ALLOWED = "Your roles do not allow this.";
const TOO_MANY = "Too many requests. Try again in a moment.";

export type DeleteFailure = { message: string; reread: boolean; gone: boolean };

/** The words for a delete that did not go through. A prompt that is already gone is gone: that is what was asked. */
export function deleteFailureOf(problem: Pick<Problem, "status" | "code">): DeleteFailure {
  const { status, code } = problem;
  if (code === "stale_revision") {
    return {
      message:
        "It was changed since you opened this page. The list was read again: check it, then delete it again.",
      reread: true,
      gone: false,
    };
  }
  if (status === 404) return { message: "This prompt no longer exists.", reread: true, gone: true };
  if (status === 0 || status >= 500) {
    return {
      message:
        "Your prompt could not be deleted just now, and it may or may not be gone. The list was read again: check it.",
      reread: true,
      gone: false,
    };
  }
  if (status === 403) {
    return { message: "Your roles grant no access to this app.", reread: false, gone: false };
  }
  if (status === 401) return { message: SIGNED_OUT, reread: false, gone: false };
  if (status === 429) return { message: TOO_MANY, reread: false, gone: false };
  return {
    message: "Your prompt could not be deleted: the server did not accept it.",
    reread: false,
    gone: false,
  };
}

export type ReadFailure = { message: string; reread: boolean; gone: boolean };

/** The words for a `getPrompt` that did not answer. A 404 is a prompt that is gone or no longer shared with the person. */
export function readFailureOf(problem: Pick<Problem, "status">): ReadFailure {
  if (problem.status === 404) {
    return {
      message: "This prompt no longer exists, or is no longer shared with you.",
      reread: true,
      gone: true,
    };
  }
  if (problem.status === 403) {
    return { message: "Your roles grant no access to this app.", reread: false, gone: false };
  }
  return {
    message: "The prompt could not be loaded just now. Try again in a moment.",
    reread: false,
    gone: false,
  };
}
