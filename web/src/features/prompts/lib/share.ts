import type { ApiPromptSummary, ApiUpdatePrompt } from "@/lib/api/types";
import { isFrozen } from "./library";

/*
 * The pure part of the sharing control (docs/api/prompts-v1.md, "The audience"): which control a row offers, what the
 * dialog's field holds and what is sent. **No rule is enforced here**: nothing refuses a role, and the server judges every
 * value it is given (`failures.ts` says what it answered). The one hint drawn is that `*` beside a role will be refused,
 * and the one value kept from being lost is a stored role the comma-separated field cannot show (`audienceIn`).
 */

/** What an own prompt's row offers: `share` (private), `change` (shared), `stop` (shared, and `prompts.share` is gone). */
export type SharingControl = "share" | "change" | "stop";

/**
 * The control of a row, or none. `permitted` is whether `getMe` lists `prompts.share`: null while it is unknown, and
 * then nothing is offered (the server decides, and a refusal is said where the action was). A prompt the person shared
 * and may no longer change offers **Stop sharing** and nothing else (`isFrozen`).
 */
export function sharingControl(
  prompt: Pick<ApiPromptSummary, "own" | "audience">,
  permitted: boolean | null,
): SharingControl | null {
  if (!prompt.own || permitted === null) return null;
  if ((prompt.audience?.length ?? 0) === 0) return permitted ? "share" : null;
  return isFrozen(prompt, permitted) ? "stop" : "change";
}

/** The roles the field names: split on commas, trimmed, the empty ones dropped, each once, in the order typed. */
export function audienceOf(text: string): string[] {
  const values: string[] = [];
  for (const part of text.split(",")) {
    const value = part.trim();
    if (value !== "" && !values.includes(value)) values.push(value);
  }
  return values;
}

/** What the field holds for an audience: the server's values as it sorted them, separated by commas. */
export const audienceField = (audience: readonly string[] | undefined): string =>
  (audience ?? []).join(", ");

/**
 * Whether the field can hold this audience at all: a role value may hold a comma (the contract allows every byte
 * `0x21` to `0x7E`, and the core's `is_role_value` with it), and the comma is what this field separates roles with.
 */
export const audienceFits = (audience: readonly string[] | undefined): boolean =>
  (audience ?? []).every((role) => !role.includes(","));

/**
 * What the field means: the roles it holds, or **the audience as it was** while the text is exactly what that audience
 * renders to. So opening the dialog and saving changes nothing, even for a role the field cannot show (one with a comma,
 * `audienceFits`); as soon as the text differs, it is read as typed.
 */
export function audienceIn(text: string, stored: readonly string[]): string[] {
  return text === audienceField(stored) ? [...stored] : audienceOf(text);
}

/** "Everyone", "Only you", or the roles. A mixed `*` is the roles with Everyone first; the server refuses it. */
export function audienceWords(audience: readonly string[] | undefined): string {
  if (!audience || audience.length === 0) return "Only you";
  if (audience.includes("*")) {
    if (audience.length === 1) return "Everyone";
    return `Everyone and ${audience.filter((role) => role !== "*").join(", ")}`;
  }
  return audience.join(", ");
}

/** The line under the field: who can read the prompt, from what the field holds now. */
export function audienceSentence(audience: readonly string[] | undefined): string {
  if (!audience || audience.length === 0) return "Only you can read it.";
  if (audience.includes("*")) {
    return audience.length === 1
      ? "Everyone can read it."
      : "“*” is only allowed alone; the server will refuse this.";
  }
  return `Everyone with one of these roles can read it: ${audience.join(", ")}.`;
}

/**
 * `updatePrompt`'s body for a share: the revision the dialog was opened at (or last read), and the audience. **No other
 * member**: a share never changes a title, a text or a question, and a member left out is a member left alone.
 */
export const buildShare = (revision: string, audience: readonly string[]): ApiUpdatePrompt => ({
  revision,
  audience: [...audience],
});

/** What the page says after a share went through. */
export const shareNotice = (title: string, audience: readonly string[]): string =>
  audience.length === 0
    ? `“${title}” is private again.`
    : `“${title}” is shared with ${audienceWords(audience)}.`;
