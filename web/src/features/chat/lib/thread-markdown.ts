import type { components } from "@/lib/api/schema";
import { answerLines, choicesOf, readAnswers, readNote } from "./a2ui/choices";
import { placeholderOf, surfaceComponents } from "./a2ui/placeholder";

/*
 * A thread as one Markdown file: the mapping of docs/api/thread-markdown-v1.md, which any client follows to produce the same
 * file. Pure: it reads the document `GET /api/threads/{id}/export` answers (the same data as Export JSON) and nothing else,
 * not a clock, not the stream, not a second request. Everything it writes the JSON export also holds.
 */

type ThreadExport = components["schemas"]["ThreadExport"];
type Event = components["schemas"]["Event"];
type Rec = Record<string, unknown>;

const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The name of the file: `thread-<id>.md`, with anything in the id that is not a plain name character dropped. */
export function markdownFilename(threadId: string): string {
  const id = threadId.replace(/[^A-Za-z0-9-]/g, "");
  return `thread-${id === "" ? "export" : id}.md`;
}

/** Text of one line for a place where Markdown would act on it: spaces collapsed, and `\ ` * _ [ ] < > &` escaped. */
export function inline(text: string): string {
  return text
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[\\`*_[\]<>&]/g, "\\$&");
}

/** A code span for untrusted text: the delimiter is longer than any run of backticks inside it. */
export function codeSpan(text: string): string {
  const one = text.replace(/\s+/g, " ").trim();
  const longest = Math.max(0, ...(one.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  const pad = one.startsWith("`") || one.endsWith("`") ? " " : "";
  return `${fence}${pad}${one}${pad}${fence}`;
}

/** The closing line a message needs when it leaves a code fence open, else nothing (CommonMark's fences). */
function openFence(text: string): string | null {
  let open: { char: string; length: number } | null = null;
  for (const line of text.split("\n")) {
    const m = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (!m) continue;
    const run = m[1] as string;
    const rest = m[2] as string;
    const char = run[0] as string;
    if (open === null) {
      if (char === "`" && rest.includes("`")) continue;
      open = { char, length: run.length };
    } else if (char === open.char && run.length >= open.length && rest.trim() === "") {
      open = null;
    }
  }
  return open === null ? null : open.char.repeat(open.length);
}

/** A message as written: LF line endings, no blank lines around it, and no code fence left open to swallow what follows. */
export function messageBody(text: string): string {
  const body = text
    .replace(/\r\n?/g, "\n")
    .replace(/^(?:[ \t]*\n)+/, "")
    .replace(/\s+$/, "");
  const closing = openFence(body);
  return closing === null ? body : `${body}\n${closing}`;
}

/** `2026-11-14T19:09:33Z`: the instant in UTC, to the second; the text itself when it is not a date. */
export function utc(iso: string): string {
  const t = new Date(iso);
  return Number.isNaN(t.getTime()) ? iso : t.toISOString().replace(/\.\d{3}Z$/, "Z");
}

const MAX_ARTIFACT_LINE = 200;

/** One line for an artifact event: a kept file by its name, anything else by its name, its text when short and its `https` link. */
function artifactLine(data: Rec): string {
  const file = isRecord(data.file) ? data.file : undefined;
  const name = str(data.name) ?? "artifact";
  if (file) {
    const shown = str(file.filename) ?? name;
    const mime = str(data.mimeType);
    const size = typeof file.size === "number" ? `${file.size} bytes` : undefined;
    const detail = [mime, size].filter((x): x is string => x !== undefined).join(", ");
    return `- File: ${codeSpan(shown)}${detail ? ` (${detail})` : ""}`;
  }
  const parts = [`Artifact: ${inline(name)}`];
  const text = str(data.text)?.trim();
  if (text && !text.includes("\n") && text.length <= MAX_ARTIFACT_LINE) parts.push(inline(text));
  const uri = str(data.uri);
  if (uri && /^https?:\/\/[^\s<>]+$/.test(uri)) parts.push(`<${uri}>`);
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
 * title, then a `## Person` or `## <agent>` section for each turn of the thread's own log (a fork carries its parent's events up to
 * the fork, then its own, so one branch is one file), the answer of an agent turn as the agent wrote it, and one line for each file,
 * artifact and interface.
 */
export function threadToMarkdown(doc: ThreadExport): string {
  const sections: string[] = [];
  const ops: unknown[] = [];
  const shown = new Set<string>();
  let turn: Turn | null = null;
  const fallback = doc.thread.target.agentId;

  const flush = () => {
    if (!turn) return;
    const t = turn;
    turn = null;
    const blocks: string[] = [];
    const steps = [...t.steps.values()].length;
    if (steps > 0) {
      const failed = [...t.steps.values()].filter((s) => s.state === "failed").length;
      blocks.push(
        `*${steps} ${steps === 1 ? "step" : "steps"}${failed > 0 ? `, ${failed} failed` : ""}.*`,
      );
    }
    const texts = t.messages.filter((m) => m.text.trim() !== "");
    const answer =
      texts.findLast((m) => m.purpose === "answer") ??
      texts.findLast((m) => m.purpose === undefined);
    if (answer) blocks.push(messageBody(answer.text));
    const known = surfaceComponents(ops);
    for (const id of t.surfaces) {
      blocks.push(`*${placeholderOf(known.get(id) ?? [])}*`);
      const questions = choicesOf(ops, id)?.questions ?? [];
      if (questions.length > 0)
        blocks.push(questions.map((q) => `- ${inline(q.question)}`).join("\n"));
    }
    if (t.artifacts.length > 0) blocks.push(t.artifacts.join("\n"));
    for (const problem of t.problems) blocks.push(problem);
    if (blocks.length === 0) return;
    sections.push([`## ${inline(t.name ?? fallback)}`, ...blocks].join("\n\n"));
  };
  const person = (...blocks: string[]) => {
    flush();
    sections.push(["## Person", ...blocks].join("\n\n"));
  };
  const agentTurn = (event: Event): Turn => {
    const t = turn ?? newTurn();
    turn = t;
    if (event.actor.type === "agent" && t.name === undefined && event.actor.name) {
      t.name = event.actor.name;
    }
    return t;
  };

  for (const event of doc.events) {
    const data: Rec = isRecord(event.data) ? event.data : {};
    switch (event.kind) {
      case "user_message":
        person(messageBody(str(data.text) ?? ""));
        break;
      case "ui_action": {
        const answers = readAnswers(data.context);
        if (!answers) {
          person(`*Used ${codeSpan(str(data.name) ?? "an action")} on an interface.*`);
          break;
        }
        const surfaceId = str(data.surfaceId) ?? "";
        const spec = choicesOf(ops, surfaceId, str(data.sourceComponentId));
        const lines = answerLines(answers, spec?.questions).map((line) => {
          const given = [
            ...line.chosen,
            ...(line.other !== undefined ? [`Other: ${line.other}`] : []),
            ...(line.text !== undefined ? [line.text] : []),
          ].join(", ");
          return `- **${inline(line.question)}** ${given === "" ? "No answer" : inline(given)}`;
        });
        const note = readNote(data.context);
        if (note !== undefined) lines.push(`- **Note** ${inline(note)}`);
        person(lines.join("\n"));
        break;
      }
      case "thread_forked": {
        flush();
        const from = str(data.title);
        const title = from ? ` "${inline(from)}"` : "";
        sections.push(
          data.kind === "edit"
            ? `*From here this is a new version: a message of the chat${title} was edited.*`
            : `*The conversation above was copied from the chat${title}; what follows was said here.*`,
        );
        break;
      }
      case "agent_message": {
        const t = agentTurn(event);
        const id = str(data.messageId);
        const next: Message = { id, text: str(data.text) ?? "", purpose: str(data.purpose) };
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
        const operations = Array.isArray(data.operations) ? data.operations : [];
        ops.push(...operations);
        for (const id of surfaceComponents(operations).keys()) {
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
        if (message) agentTurn(event).problems.push(`*Error: ${inline(message)}*`);
        break;
      }
      case "agent_status": {
        const t = agentTurn(event);
        if (data.status === "failed") {
          const detail = str(data.detail);
          t.problems.push(`*The agent failed${detail ? `: ${inline(detail)}` : "."}*`);
        } else if (data.status === "canceled") {
          t.problems.push("*The turn was stopped.*");
        }
        break;
      }
      default:
        break;
    }
  }
  flush();

  const title = doc.thread.title.replace(/\s+/g, " ").trim();
  const front = [
    "---",
    "format: another-agentic-system/thread-markdown",
    "version: 1",
    `title: ${JSON.stringify(title)}`,
    ...(doc.thread.description
      ? [`description: ${JSON.stringify(doc.thread.description.replace(/\s+/g, " ").trim())}`]
      : []),
    `agent: ${JSON.stringify(doc.thread.target.agentId)}`,
    `thread: ${JSON.stringify(doc.thread.id)}`,
    `exported: ${utc(doc.exportedAt)}`,
    "---",
  ].join("\n");
  const out = [front, `# ${inline(title === "" ? "Untitled chat" : title)}`, ...sections];
  if (doc.eventsTruncated) {
    out.push(
      `*The chat is longer than this file: it holds the first ${doc.events.length} events.*`,
    );
  }
  return `${out.join("\n\n")}\n`;
}
