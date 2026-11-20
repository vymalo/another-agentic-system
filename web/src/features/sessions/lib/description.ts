/**
 * The description of a subagent a person starts, judged as the server judges it
 * (`check_start_description`, `orchestrator/crates/app/src/app/sessions.rs`, `startChildSession`): the
 * text with the space around it cut, 1 to 80 characters **counted in code points** (an emoji is one,
 * never two UTF-16 units), on one line, with no control character and no Unicode format character.
 *
 * The screen checks it so that a person is told before the request is made; the server decides.
 */

/** The longest description, in characters (code points). */
export const MAX_START_DESCRIPTION = 80;

/** Whether Rust's `char::is_whitespace` holds for `c`: what the server's `trim` cuts (JavaScript's also cuts U+FEFF). */
function isSpace(c: number): boolean {
  return (
    (c >= 0x09 && c <= 0x0d) ||
    c === 0x20 ||
    c === 0x85 ||
    c === 0xa0 ||
    c === 0x1680 ||
    (c >= 0x2000 && c <= 0x200a) ||
    c === 0x2028 ||
    c === 0x2029 ||
    c === 0x202f ||
    c === 0x205f ||
    c === 0x3000
  );
}

/** Whether `c` reads as nothing or reorders what is around it: refused as a control character is. */
function isFormat(c: number): boolean {
  return (
    c === 0xad ||
    (c >= 0x200b && c <= 0x200f) ||
    (c >= 0x202a && c <= 0x202e) ||
    (c >= 0x2060 && c <= 0x2064) ||
    (c >= 0x2066 && c <= 0x2069) ||
    c === 0xfeff
  );
}

const CONTROL = /\p{Cc}/u;

/** `text` without the white space at both ends, as the server cuts it. */
function trimmed(text: string): string {
  const points = [...text];
  let from = 0;
  let to = points.length;
  while (from < to && isSpace(points[from]?.codePointAt(0) ?? 0)) from += 1;
  while (to > from && isSpace(points[to - 1]?.codePointAt(0) ?? 0)) to -= 1;
  return points.slice(from, to).join("");
}

/** The number of characters of `text`, counting code points and never UTF-16 units. */
export function codePointLength(text: string): number {
  let n = 0;
  for (const _ of text) n += 1;
  return n;
}

export type DescriptionCheck =
  | {
      ok: true /** The text as the server will read it (trimmed). */;
      value: string;
      length: number;
    }
  | { ok: false; reason: "blank" }
  | { ok: false; reason: "unreadable" }
  | { ok: false; reason: "too_long"; length: number };

/** Judges `raw` in the server's order: blank, then control or format characters, then the length. */
export function checkStartDescription(raw: string): DescriptionCheck {
  const value = trimmed(raw);
  if (value === "") return { ok: false, reason: "blank" };
  if (CONTROL.test(value) || [...value].some((c) => isFormat(c.codePointAt(0) ?? 0))) {
    return { ok: false, reason: "unreadable" };
  }
  const length = codePointLength(value);
  if (length > MAX_START_DESCRIPTION) return { ok: false, reason: "too_long", length };
  return { ok: true, value, length };
}
