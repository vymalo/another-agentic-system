import { inputCut, type StepInput } from "./agui/vymalo";

/*
 * What a tool step is called and what it was called with, for the one line a row keeps. An MCP tool
 * reaches the page as `server__tool` (`search__web_search`): a person reads "Web search", from the
 * server "search", and the one argument that says what it did ("Stephane Segning"). Pure.
 */

/** `server__tool`: what an agent calls an MCP server's tool. No space, no other separator. */
const SERVER_TOOL = /^([A-Za-z0-9][A-Za-z0-9-]*)__([A-Za-z0-9][A-Za-z0-9_.-]*)$/;

export type ToolName = {
  /** "Web search" for `search__web_search`; the label itself when it is not a `server__tool`. */
  title: string;
  /** `search`: the MCP server it belongs to. */
  server?: string;
};

/** `web_search` as words: "Web search". */
function words(tool: string): string {
  const spaced = tool.replace(/[_.-]+/g, " ").trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

export function toolName(label: string): ToolName {
  const match = SERVER_TOOL.exec(label);
  if (!match?.[1] || !match[2]) return { title: label };
  return { title: words(match[2]), server: match[1] };
}

/** The longest argument a row quotes. */
export const PREVIEW_MAX = 60;

/** The arguments that say what a call was about, best first. */
const PREFERRED = [
  "query",
  "q",
  "url",
  "path",
  "file_path",
  "file",
  "command",
  "pattern",
  "name",
  "id",
];

const oneLine = (text: string): string => text.replace(/\s+/g, " ").trim();

/**
 * One short phrase for what a call was about: the first argument of the usual names that is
 * text, else the first text there is. Nothing for an input that was not kept, for a credential
 * and for arguments that are not text.
 */
export function inputPreview(input: StepInput | undefined): string | undefined {
  if (!input || inputCut(input)) return undefined;
  const usable = (v: unknown): string | undefined => {
    if (typeof v !== "string") return undefined;
    const line = oneLine(v);
    return line === "" || line === "[redacted]" ? undefined : line;
  };
  let found: string | undefined;
  for (const key of PREFERRED) {
    found = usable(input[key]);
    if (found) break;
  }
  if (!found) {
    for (const value of Object.values(input)) {
      found = usable(value);
      if (found) break;
    }
  }
  if (!found) return undefined;
  return found.length > PREVIEW_MAX ? `${found.slice(0, PREVIEW_MAX - 1)}…` : found;
}

const UTF8 = new TextEncoder();

/** `512 bytes`, `1.5 KiB`, `41 KiB`, `1.2 MiB`: a size for a person. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "0 bytes";
  if (bytes < 1024) return `${Math.round(bytes)} ${Math.round(bytes) === 1 ? "byte" : "bytes"}`;
  const kib = bytes / 1024;
  if (kib < 1024) return `${kib < 10 ? kib.toFixed(1).replace(/\.0$/, "") : Math.round(kib)} KiB`;
  const mib = kib / 1024;
  return `${mib < 10 ? mib.toFixed(1).replace(/\.0$/, "") : Math.round(mib)} MiB`;
}

/**
 * How much of a cut output is not there: the size of all of it (`bytes`) less the text that was
 * kept. Undefined when the output is not cut, or when it did not say its size.
 */
export function notKept(output: {
  text: string;
  truncated?: true;
  bytes?: number;
}): number | undefined {
  if (!output.truncated || output.bytes === undefined) return undefined;
  return Math.max(0, output.bytes - UTF8.encode(output.text).length);
}
