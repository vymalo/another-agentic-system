import type {
  ApiChoicesQuestion,
  ApiCreatePrompt,
  ApiPrompt,
  ApiPromptSummary,
  ApiUpdatePrompt,
} from "@/lib/api/types";
import { type FormQuestion, fromForm, toForm } from "./editor";

/*
 * The pure part of the prompt library page: order, who owns what, the words of a card, and the bodies of the two writes. Nothing
 * here is stored on the device (docs/decisions/0067: nothing a person owns lives only in one client), and no rule of the
 * contract is applied: the server judges every value, and `failures.ts` says what it answered.
 */

export type PromptFields = {
  title: string;
  name: string;
  description: string;
  body: string;
  questions: FormQuestion[];
};

/** The contract tells a client to sort for display: by `name`, then `id` (the server orders by `id`). */
const byName = (a: { name: string; id: string }, b: { name: string; id: string }) =>
  a.name < b.name ? -1 : a.name > b.name ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0;

export const sortPrompts = <T extends { name: string; id: string }>(list: readonly T[]): T[] =>
  [...list].sort(byName);

/** What the person owns, and what is shared with them, each sorted. */
export function splitPrompts(list: readonly ApiPromptSummary[]): {
  own: ApiPromptSummary[];
  shared: ApiPromptSummary[];
} {
  return {
    own: sortPrompts(list.filter((p) => p.own)),
    shared: sortPrompts(list.filter((p) => !p.own)),
  };
}

/**
 * A prompt the person shared and can no longer change: `prompts.share` is known to be missing. `permitted` is null while `me`
 * is unknown, and then nothing is frozen: the server decides and says so (403 `forbidden`). Only a shared prompt can be.
 */
export function isFrozen(
  prompt: Pick<ApiPromptSummary, "own" | "audience">,
  permitted: boolean | null,
): boolean {
  return prompt.own && (prompt.audience?.length ?? 0) > 0 && permitted === false;
}

/** `/name` of a title: lower-case ASCII, accents folded, hyphens between words, at most 32, never starting with a hyphen. */
export function slugName(title: string): string {
  return title
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+/, "")
    .slice(0, 32)
    .replace(/-+$/, "");
}

/** "Shared with: Everyone", or the roles as the server sorted them; none for a private prompt. */
export function sharedLine(audience: readonly string[] | undefined): string | null {
  if (!audience || audience.length === 0) return null;
  return `Shared with: ${audience.includes("*") ? "Everyone" : audience.join(", ")}`;
}

export const asksWords = (count: number): string =>
  count === 0 ? "Asks no questions" : count === 1 ? "Asks 1 question" : `Asks ${count} questions`;

const encoder = new TextEncoder();
/** The size of the text as the server counts it (`maxBodyBytes`). */
export const byteLength = (text: string): number => encoder.encode(text).length;

/** The fields of the form for a prompt that was read. */
export const fieldsOf = (
  p: Pick<ApiPrompt, "title" | "name" | "description" | "body" | "variables">,
): PromptFields => ({
  title: p.title,
  name: p.name,
  description: p.description,
  body: p.body,
  questions: toForm(p.variables),
});

/** The names of the questions as they would be sent, in order: what a refusal of "unused" names. */
export const questionIds = (questions: readonly FormQuestion[]): string[] =>
  questions.map((q) => (q.kept ? q.id : q.fields.id.trim()));

/** A key-order-free comparison: the server may write the members of an object in another order than a form did. */
function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every(
    (k) =>
      Object.hasOwn(b, k) &&
      deepEqual((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]),
  );
}

/** `createPrompt`'s body. The server never trims, so the title, the name and the description are trimmed here; the text is as typed. */
export function buildCreate(f: PromptFields): ApiCreatePrompt {
  const body: ApiCreatePrompt = { name: f.name.trim(), title: f.title.trim(), body: f.body };
  const description = f.description.trim();
  if (description !== "") body.description = description;
  const variables = fromForm(f.questions) as ApiChoicesQuestion[];
  if (variables.length > 0) body.variables = variables;
  return body;
}

/**
 * `updatePrompt`'s body for an edit: only what differs, with the `revision` the person was looking at, or null when nothing
 * differs (no request is better than one the server answers with the same prompt). `variables` goes only when the list the form
 * comes to differs from what was read, and then whole (a member given replaces the whole member). **No `audience`, ever**: sharing
 * is another control, and an edit never changes who a prompt is shared with.
 */
export function buildChange(was: ApiPrompt, f: PromptFields): ApiUpdatePrompt | null {
  const body: ApiUpdatePrompt = { revision: was.revision };
  const name = f.name.trim();
  const title = f.title.trim();
  const description = f.description.trim();
  if (name !== was.name) body.name = name;
  if (title !== was.title) body.title = title;
  if (description !== was.description) body.description = description;
  if (f.body !== was.body) body.body = f.body;
  const variables = fromForm(f.questions);
  if (!deepEqual(variables, was.variables)) body.variables = variables as ApiChoicesQuestion[];
  return Object.keys(body).length === 1 ? null : body;
}
