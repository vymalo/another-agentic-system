import type { components } from "@/lib/api/schema";
import { answerLines, choicesOf, readAnswers, readNote } from "./a2ui/choices";
import { placeholderOf, SurfaceLog } from "./a2ui/placeholder";

/*
 * A thread as one Markdown file: the mapping of docs/api/thread-markdown-v1.md, which any client follows to produce the same
 * file. Pure: it reads the document `GET /api/threads/{id}/export` answers (the same data as Export JSON) and nothing else,
 * not a clock, not the stream, not a second request. Everything it writes the JSON export also holds.
 *
 * What a participant wrote is only ever written inside a block quote (`quoted`), so it cannot become the file's own structure
 * (a heading, an HTML block, an open code fence). The page holds the wording below to the code: `thread-markdown.test.ts` reads it.
 */

type ThreadExport = components["schemas"]["ThreadExport"];
type Event = components["schemas"]["Event"];
type Rec = Record<string, unknown>;

const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The words the exporter writes itself. The page quotes every one of them; a test holds the page to these. */
export const WORDS = {
  /** The members of the front matter, in order (`description` only when the thread has one). */
  frontMatter: ["format", "version", "title", "description", "agent", "thread", "exported"],
  format: "another-agentic-system/thread-markdown",
  /** The heading of the person's turns. Its `*` can never be written by an agent's name, whose `*` is always escaped. */
  person: "## *Person*",
  untitled: "Untitled chat",
  emptyMessage: "*Empty message.*",
  noAnswer: "No answer",
  note: "**Note**",
  stopped: "*The turn was stopped.*",
  failed: "*The agent failed.*",
  failedWith: "*The agent failed: <detail>*",
  error: "*Error: <message>*",
  used: "*Used <name> on an interface.*",
  step: "*1 step.*",
  steps: "*<N> steps.*",
  stepsFailed: ", <M> failed",
  forkCopy:
    '*The conversation above was copied from the chat "<title>"; what follows was said here.*',
  forkCopyUntitled:
    "*The conversation above was copied from another chat; what follows was said here.*",
  forkEdit: '*From here this is a new version: a message of the chat "<title>" was edited.*',
  forkEditUntitled: "*From here this is a new version: a message of an earlier chat was edited.*",
  truncated: "*The chat is longer than this file: it holds the first <N> events.*",
  file: "- File: ",
  artifact: "- Artifact: ",
  other: "Other: ",
} as const;

/** The longest one-line artifact text written, in Unicode scalar values (code points). */
export const MAX_ARTIFACT_LINE = 200;

/** The name of the file: `thread-<id>.md`, with anything in the id that is not a plain name character dropped. */
export function markdownFilename(threadId: string): string {
  const id = threadId.replace(/[^A-Za-z0-9-]/g, "");
  return `thread-${id === "" ? "export" : id}.md`;
}

/** A word that a Markdown reader would turn into a link by itself: an address (`@`), `www.`, or a scheme and its colon. */
const LINKABLE = /@|(?:^|[^A-Za-z0-9])www\.|(?:https?|ftp|mailto|xmpp):/i;

/**
 * A short string a participant wrote, as one line of the file's own text: whitespace runs (space, tab, CR, LF) become one space,
 * the ends are trimmed, and what Markdown would act on is backslash-escaped (`\ ` * _ ~ [ ] < > &`). A word that a reader would
 * make a link by itself (an address, `www.`, a scheme and its colon) is written as a code span instead: GFM finds those in the
 * text after the escapes are read, so an escape cannot stop it.
 */
export function inline(text: string): string {
  return text
    .replace(/[ \t\r\n]+/g, " ")
    .replace(/^ | $/g, "")
    .split(" ")
    .map((word) => (LINKABLE.test(word) ? codeSpan(word) : word.replace(/[\\`*_~[\]<>&]/g, "\\$&")))
    .join(" ");
}

/** A code span for untrusted text: the delimiter is longer than any run of backticks inside it. */
export function codeSpan(text: string): string {
  const one = text.replace(/[ \t\r\n]+/g, " ").replace(/^ | $/g, "");
  const longest = Math.max(0, ...(one.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  const pad = one.startsWith("`") || one.endsWith("`") ? " " : "";
  return `${fence}${pad}${one}${pad}${fence}`;
}

const isBlank = (text: string): boolean => /^[ \t\r\n]*$/.test(text);

/**
 * A message as the file keeps it: CR and CRLF are LF, the lines at the start that hold only spaces and tabs are dropped, and the
 * spaces, tabs and line feeds at the end are dropped. Nothing else is touched (trailing spaces inside the text stay). `null` for
 * a message with nothing else in it.
 */
export function messageBody(text: string): string | null {
  const body = text
    .replace(/\r\n?/g, "\n")
    .replace(/^(?:[ \t]*\n)+/, "")
    .replace(/[ \t\n]+$/, "");
  return body === "" ? null : body;
}

/** Every line of the body behind `> ` (an empty line behind a bare `>`): the one way a participant's text enters the file. */
export function quoted(body: string): string {
  return body
    .split("\n")
    .map((line) => (line === "" ? ">" : `> ${line}`))
    .join("\n");
}

/** The body a quote holds: a line that is exactly `>` is empty, any other line loses its first two characters, `> `. */
export function unquoted(block: string): string {
  return block
    .split("\n")
    .map((line) => (line === ">" ? "" : line.slice(2)))
    .join("\n");
}

/** `2026-11-14T19:09:33Z`: the instant in UTC, cut to the second (never rounded); the text itself when it is not a date. */
export function utc(iso: string): string {
  const t = new Date(iso);
  return Number.isNaN(t.getTime()) ? iso : t.toISOString().replace(/\.\d{3}Z$/, "Z");
}

const scalars = (text: string): number => [...text].length;

/** An `http` or `https` address (either case) with no character below U+0021, no U+007F and no `<` or `>`. */
function isLinkable(uri: string): boolean {
  if (!/^https?:\/\/./i.test(uri)) return false;
  for (const ch of uri) {
    const code = ch.codePointAt(0) ?? 0;
    if (code < 0x21 || code === 0x7f || ch === "<" || ch === ">") return false;
  }
  return true;
}

/** One line for an artifact event: a kept file by its name, anything else by its name, its text when short and its `https` link. */
function artifactLine(data: Rec): string {
  const file = isRecord(data.file) ? data.file : undefined;
  const name = str(data.name) ?? "artifact";
  if (file) {
    // a kept file wins over a `uri`: no link, whatever the agent said
    const shown = str(file.filename) ?? name;
    const mime = str(data.mimeType);
    const size = typeof file.size === "number" ? `${file.size} bytes` : undefined;
    const detail = [mime, size].filter((x): x is string => x !== undefined).join(", ");
    return `${WORDS.file}${codeSpan(shown)}${detail ? ` (${inline(detail)})` : ""}`;
  }
  const parts = [`${WORDS.artifact.slice(2)}${inline(name)}`];
  const text = str(data.text)?.trim();
  if (text && !/[\n\r]/.test(text) && scalars(text) <= MAX_ARTIFACT_LINE) parts.push(inline(text));
  const uri = str(data.uri);
  if (uri && isLinkable(uri)) parts.push(`<${uri}>`);
  return `- ${parts.join(" — ")}`;
}

type Message = { id: string | undefined; text: string; purpose: string | undefined };

type Turn = {
  name: string | undefined;
  messages: Message[];
  steps: Map<string, { state: string }>;
  surfaces: string[];
  artifacts: string[];
  problems: string[];
};

const newTurn = (): Turn => ({
  name: undefined,
  messages: [],
  steps: new Map(),
  surfaces: [],
  artifacts: [],
  problems: [],
});

/**
 * The thread as Markdown. See `docs/api/thread-markdown-v1.md` for what is written and what is left out and why: front matter, the
 * title, then a `## *Person*` or `## <agent>` section for each turn of the thread's own log (a fork carries its parent's events up to
 * the fork, then its own, so one branch is one file), what a participant wrote inside a block quote, and one line for each file,
 * artifact and interface.
 */
export function threadToMarkdown(doc: ThreadExport): string {
  const sections: string[] = [];
  const surfaces = new SurfaceLog();
  const shown = new Set<string>();
  let turn: Turn | null = null;
  const fallback = doc.thread.target.agentId;

  const flush = () => {
    if (!turn) return;
    const t = turn;
    turn = null;
    const blocks: string[] = [];
    const steps = t.steps.size;
    if (steps > 0) {
      const failed = [...t.steps.values()].filter((s) => s.state === "failed").length;
      const count = steps === 1 ? WORDS.step : WORDS.steps.replace("<N>", String(steps));
      blocks.push(
        failed > 0
          ? count.replace(/\.\*$/, `${WORDS.stepsFailed.replace("<M>", String(failed))}.*`)
          : count,
      );
    }
    const texts = t.messages.filter((m) => !isBlank(m.text));
    const answer =
      texts.findLast((m) => m.purpose === "answer") ??
      texts.findLast((m) => m.purpose === undefined);
    const body = answer ? messageBody(answer.text) : null;
    if (body !== null) blocks.push(quoted(body));
    for (const id of t.surfaces) {
      blocks.push(`*${placeholderOf(surfaces.types(id))}*`);
      const questions = choicesOf(surfaces.operations(id), id)?.questions ?? [];
      if (questions.length > 0)
        blocks.push(questions.map((q) => `- ${inline(q.question)}`).join("\n"));
    }
    if (t.artifacts.length > 0) blocks.push(t.artifacts.join("\n"));
    for (const problem of t.problems) blocks.push(problem);
    if (blocks.length === 0) return;
    const name = inline(t.name ?? fallback);
    sections.push([`## ${name === "" ? "agent" : name}`, ...blocks].join("\n\n"));
  };
  const person = (...blocks: string[]) => {
    flush();
    sections.push([WORDS.person, ...blocks].join("\n\n"));
  };
  const agentTurn = (event: Event): Turn => {
    const t = turn ?? newTurn();
    turn = t;
    if (event.actor.type === "agent" && t.name === undefined && event.actor.name) {
      t.name = event.actor.name;
    }
    return t;
  };
  const answerBlock = (text: string): string => {
    const body = messageBody(text);
    return body === null ? WORDS.noAnswer : quoted(body);
  };

  for (const event of doc.events) {
    const data: Rec = isRecord(event.data) ? event.data : {};
    switch (event.kind) {
      case "user_message": {
        const body = messageBody(str(data.text) ?? "");
        person(body === null ? WORDS.emptyMessage : quoted(body));
        break;
      }
      case "ui_action": {
        const answers = readAnswers(data.context);
        if (!answers) {
          person(WORDS.used.replace("<name>", codeSpan(str(data.name) ?? "an action")));
          break;
        }
        const surfaceId = str(data.surfaceId) ?? "";
        const spec = choicesOf(
          surfaces.operations(surfaceId),
          surfaceId,
          str(data.sourceComponentId),
        );
        const blocks: string[] = [];
        for (const line of answerLines(answers, spec?.questions)) {
          const given = [
            ...line.chosen,
            ...(line.other !== undefined ? [`${WORDS.other}${line.other}`] : []),
            ...(line.text !== undefined ? [line.text] : []),
          ].join(", ");
          blocks.push(`**${inline(line.question)}**`, answerBlock(given));
        }
        const note = readNote(data.context);
        if (note !== undefined) blocks.push(WORDS.note, answerBlock(note));
        person(...blocks);
        break;
      }
      case "thread_forked": {
        flush();
        const from = str(data.title);
        const title = from ? inline(from) : "";
        const edit = data.kind === "edit";
        sections.push(
          title === ""
            ? edit
              ? WORDS.forkEditUntitled
              : WORDS.forkCopyUntitled
            : (edit ? WORDS.forkEdit : WORDS.forkCopy).replace("<title>", () => title),
        );
        break;
      }
      case "agent_message": {
        const t = agentTurn(event);
        const id = str(data.messageId);
        const next: Message = { id, text: str(data.text) ?? "", purpose: str(data.purpose) };
        // a message written again keeps the place of its first appearance and the words of its last event
        const at = id === undefined ? -1 : t.messages.findIndex((m) => m.id === id);
        if (at >= 0) t.messages[at] = next;
        else t.messages.push(next);
        break;
      }
      case "agent_step": {
        const t = agentTurn(event);
        const id = str(data.id);
        if (id !== undefined && data.kind !== "message") {
          t.steps.set(id, { state: str(data.state) ?? "" });
        }
        break;
      }
      case "ui_surface": {
        const t = agentTurn(event);
        for (const id of surfaces.feed(data.operations)) {
          if (shown.has(id)) continue;
          shown.add(id);
          t.surfaces.push(id);
        }
        break;
      }
      case "artifact":
        agentTurn(event).artifacts.push(artifactLine(data));
        break;
      case "error": {
        const message = str(data.message);
        if (message)
          agentTurn(event).problems.push(WORDS.error.replace("<message>", () => inline(message)));
        break;
      }
      case "agent_status": {
        const t = agentTurn(event);
        if (data.status === "failed") {
          const detail = str(data.detail);
          t.problems.push(
            detail ? WORDS.failedWith.replace("<detail>", () => inline(detail)) : WORDS.failed,
          );
        } else if (data.status === "canceled") {
          t.problems.push(WORDS.stopped);
        }
        break;
      }
      default:
        break;
    }
  }
  flush();

  const title = doc.thread.title.replace(/[ \t\r\n]+/g, " ").replace(/^ | $/g, "");
  const description = doc.thread.description?.replace(/[ \t\r\n]+/g, " ").replace(/^ | $/g, "");
  const front = [
    "---",
    `format: ${WORDS.format}`,
    "version: 1",
    `title: ${JSON.stringify(title)}`,
    ...(description ? [`description: ${JSON.stringify(description)}`] : []),
    `agent: ${JSON.stringify(doc.thread.target.agentId)}`,
    `thread: ${JSON.stringify(doc.thread.id)}`,
    `exported: ${utc(doc.exportedAt)}`,
    "---",
  ].join("\n");
  const heading = inline(title === "" ? WORDS.untitled : title);
  const out = [front, `# ${heading}`, ...sections];
  if (doc.eventsTruncated) {
    out.push(WORDS.truncated.replace("<N>", String(doc.events.length)));
  }
  return `${out.join("\n\n")}\n`;
}
