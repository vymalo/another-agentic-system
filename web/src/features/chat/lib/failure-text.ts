/** A first line longer than this is cut, and the whole text goes behind the disclosure. */
export const MESSAGE_LIMIT = 240;

export type SplitFailure = {
  /** The line the callout says: the first line of the text, at most {@link MESSAGE_LIMIT} characters. */
  message: string;
  /** What the callout keeps behind "Show details", as it was written; empty when nothing is. */
  details: string;
};

/**
 * A failure's text as the callout shows it (untrusted: an agent's reason, a finding, a type checker's
 * output): the first non-empty line is the message, and everything after it is the details, kept as
 * written (indentation, code frames) apart from the blank lines at its ends. A first line longer than
 * {@link MESSAGE_LIMIT} characters is cut with an ellipsis and the whole text is the details, so
 * nothing is lost and one long line is not one wall of text either. Text of one short line has no details.
 */
export function splitFailure(text: string): SplitFailure {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const at = lines.findIndex((line) => line.trim() !== "");
  if (at < 0) return { message: "", details: "" };
  const first = (lines[at] ?? "").trim();
  const rest = lines
    .slice(at + 1)
    .join("\n")
    .replace(/^\n+/, "")
    .trimEnd();
  if ([...first].length > MESSAGE_LIMIT) {
    const cut = [...first].slice(0, MESSAGE_LIMIT).join("").trimEnd();
    return { message: `${cut}…`, details: [first, rest].filter(Boolean).join("\n") };
  }
  return { message: first, details: rest };
}
