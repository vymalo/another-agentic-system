/**
 * What the code components of the UI catalog (docs/api/ui-catalog-v1.md, version 6) show: what each
 * line of a unified diff is, what an exit code says, and where a long output folds. Pure: no React,
 * no I/O. The renderers draw what these return, and every string stays the agent's, as text.
 */

export type DiffKind = "header" | "hunk" | "add" | "remove" | "context";
export type DiffLine = { kind: DiffKind; text: string };

/** Lines of a terminal output shown before it folds behind "Show all". */
export const FOLD_LINES = 12;

const HEADER =
  /^(diff |index |new file mode|deleted file mode|old mode|new mode|similarity index|rename from|rename to|copy from|copy to|Binary files )/;
const HUNK = /^@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))? @@/;

/**
 * Each line of a unified diff, classified. A hunk header says how many old and new lines follow it,
 * so inside a hunk a line that begins `---` or `+++` is a change (a removed line that begins with two
 * dashes is not a file header); outside one the same lines are the file headers of the diff.
 */
export function diffLines(diff: string): DiffLine[] {
  const out: DiffLine[] = [];
  let oldLeft = 0;
  let newLeft = 0;
  for (const raw of diff.split("\n")) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
    const hunk = HUNK.exec(line);
    if (hunk) {
      oldLeft = Number(hunk[1] ?? 1);
      newLeft = Number(hunk[2] ?? 1);
      out.push({ kind: "hunk", text: line });
      continue;
    }
    const inHunk = oldLeft > 0 || newLeft > 0;
    if (inHunk && line.startsWith("\\")) {
      out.push({ kind: "header", text: line }); // "\ No newline at end of file"
    } else if (inHunk && line.startsWith("-")) {
      oldLeft = Math.max(0, oldLeft - 1);
      out.push({ kind: "remove", text: line.slice(1) });
    } else if (inHunk && line.startsWith("+")) {
      newLeft = Math.max(0, newLeft - 1);
      out.push({ kind: "add", text: line.slice(1) });
    } else if (inHunk) {
      oldLeft = Math.max(0, oldLeft - 1);
      newLeft = Math.max(0, newLeft - 1);
      out.push({ kind: "context", text: line.startsWith(" ") ? line.slice(1) : line });
    } else if (HEADER.test(line) || line.startsWith("--- ") || line.startsWith("+++ ")) {
      out.push({ kind: "header", text: line });
    } else if (line.startsWith("+")) {
      out.push({ kind: "add", text: line.slice(1) });
    } else if (line.startsWith("-")) {
      out.push({ kind: "remove", text: line.slice(1) });
    } else {
      out.push({ kind: "context", text: line });
    }
  }
  return out;
}

export type ExitStatus =
  | { kind: "ok"; code: 0 }
  | { kind: "failed"; code: number }
  | { kind: "unknown" };

/** Zero is ok, another code failed, and none (a command still running, or not told) is unknown. */
export function exitStatus(code: number | undefined): ExitStatus {
  if (code === undefined) return { kind: "unknown" };
  return code === 0 ? { kind: "ok", code: 0 } : { kind: "failed", code };
}

/** Where a terminal output is shown folded: its lines, whether it folds, and the lines shown first. */
export function foldOutput(output: string): { lines: number; folded: boolean; shown: string } {
  const body = output.endsWith("\n") ? output.slice(0, -1) : output;
  const all = body.split("\n");
  const folded = all.length > FOLD_LINES;
  return {
    lines: all.length,
    folded,
    shown: folded ? all.slice(0, FOLD_LINES).join("\n") : output,
  };
}

const HIDDEN =
  // biome-ignore lint/suspicious/noControlCharactersInRegex: showing them is the point
  /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

/**
 * The text as it is read: a control or direction character (the one that hides or reverses text, Trojan Source)
 * becomes its code point in words, `⟨U+202E⟩`. A tab and a line break stay. What is copied is this text too.
 */
export function visibleControls(text: string): string {
  return text.replace(
    HIDDEN,
    (c) => `\u27e8U+${c.charCodeAt(0).toString(16).toUpperCase().padStart(4, "0")}\u27e9`,
  );
}
