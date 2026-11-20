import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { gfm } from "micromark-extension-gfm";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  markdownFilename,
  messageBody,
  threadToMarkdown,
  unquoted,
  WORDS,
} from "../src/features/chat/lib/thread-markdown";
import type { components } from "../src/lib/api/schema";
import { postRun } from "./agui-client";
import { createMockServer } from "./server";
import {
  GOLDEN_AT,
  GOLDEN_DIR,
  GOLDEN_ID,
  normalizeExport,
  updateGolden,
} from "./thread-markdown-golden";
import { HAND } from "./thread-markdown-vectors";

/**
 * The conformance vectors of docs/api/thread-markdown-v1.md, in `docs/api/examples/thread-markdown/`: each `<name>.export.json`
 * is an export and `<name>.md` the file a client must write from it. Some are threads the mock plays (a test checks that the mock
 * still plays the checked-in input, so the two cannot drift), the others are written by hand for one rule each
 * (`thread-markdown-vectors.ts`, checked the same way). `UPDATE_GOLDEN=1` rewrites them, and never in CI.
 */

type ThreadExport = components["schemas"]["ThreadExport"];

const server = createMockServer({ stepMs: 2, keepaliveMs: 1000 });
let base = "";
beforeAll(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(async () => {
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

const json = async (url: string, init?: RequestInit) => (await fetch(`${base}${url}`, init)).json();
const waitFor = async (id: string, state: string) => {
  for (let i = 0; i < 1000; i++) {
    if ((await json(`/api/threads/${id}`)).state === state) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`thread ${id} never reached ${state}`);
};
const say = async (id: string, n: number, content: string) => {
  const res = await postRun(base, "reviewer", {
    threadId: id,
    runId: `run-${n}`,
    messages: [{ id: `evt-${n}`, role: "user", content }],
  });
  expect(res.status).toBe(200);
  await res.text();
  await waitFor(id, "done");
};
const exported = (id: string): Promise<ThreadExport> => json(`/api/threads/${id}/export`);

const PLAYED: Record<string, () => Promise<ThreadExport>> = {
  "plain-chat": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "talk");
    await say(id, 6, "steps go");
    return exported(id);
  },
  "code-math": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "code-math please");
    return exported(id);
  },
  "form-answer": async () => {
    const id = crypto.randomUUID();
    const first = await postRun(base, "reviewer", {
      threadId: id,
      runId: "run-1",
      messages: [{ id: "evt-1", role: "user", content: "choices now" }],
    });
    await first.text();
    await waitFor(id, "blocked");
    const answer = await postRun(base, "reviewer", {
      threadId: id,
      runId: "run-2",
      messages: [],
      forwardedProps: {
        a2uiAction: {
          userAction: {
            name: "answer",
            surfaceId: "s1",
            sourceComponentId: "pick",
            context: {
              answers: [
                { id: "db", values: ["pg"] },
                { id: "auth", values: ["keycloak"] },
              ],
              note: "Use *our* realm\nand the second line",
            },
          },
        },
      },
    });
    await answer.text();
    await waitFor(id, "done");
    return exported(id);
  },
  "fork-regenerate": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "echo first");
    await say(id, 6, "echo second");
    const second = (await exported(id)).events.filter((e) => e.kind === "user_message")[1];
    const res = await fetch(`${base}/api/threads/${id}/fork`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ replace: second?.seq, text: "echo second", messageId: "evt-regen" }),
    });
    expect(res.status).toBeLessThan(300);
    const fork = (await res.json()) as { id: string };
    await waitFor(fork.id, "done");
    return exported(fork.id);
  },
  "kept-file": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "file make a chart");
    return exported(id);
  },
};

const input = (name: string) => path.join(GOLDEN_DIR, `${name}.export.json`);
const output = (name: string) => path.join(GOLDEN_DIR, `${name}.md`);
const pretty = (doc: unknown) => `${JSON.stringify(doc, null, 2)}\n`;
const read = (name: string): ThreadExport => JSON.parse(readFileSync(input(name), "utf8"));
const NAMES = [...Object.keys(PLAYED), ...Object.keys(HAND)];

describe("the vectors on disk", () => {
  it("are exactly the ones this test knows, each with its input and its file", () => {
    const files = readdirSync(GOLDEN_DIR).filter((f) => f.endsWith(".md") || f.endsWith(".json"));
    expect(files.sort()).toEqual(NAMES.flatMap((n) => [`${n}.export.json`, `${n}.md`]).sort());
  });

  for (const [name, play] of Object.entries(PLAYED)) {
    it(`${name}: the mock still plays the checked-in input`, async () => {
      const doc = normalizeExport(await play());
      if (updateGolden(process.env)) {
        mkdirSync(GOLDEN_DIR, { recursive: true });
        writeFileSync(input(name), pretty(doc));
      }
      expect(doc).toEqual(read(name));
    });
  }

  for (const [name, doc] of Object.entries(HAND)) {
    it(`${name}: the checked-in input is the one written for it`, () => {
      const fixed = normalizeExport(doc);
      if (updateGolden(process.env)) {
        mkdirSync(GOLDEN_DIR, { recursive: true });
        writeFileSync(input(name), pretty(fixed));
      }
      expect(read(name)).toEqual(fixed);
    });
  }

  for (const name of NAMES) {
    it(`${name}: the input makes exactly the checked-in file`, () => {
      const markdown = threadToMarkdown(read(name));
      if (updateGolden(process.env)) writeFileSync(output(name), markdown);
      expect(markdown).toBe(readFileSync(output(name), "utf8"));
    });
  }
});

describe("the golden flag", () => {
  it("is off unless asked for, and refused in CI", () => {
    expect(updateGolden({})).toBe(false);
    expect(updateGolden({ UPDATE_GOLDEN: "" })).toBe(false);
    expect(updateGolden({ UPDATE_GOLDEN: "0" })).toBe(false);
    expect(updateGolden({ UPDATE_GOLDEN: "1" })).toBe(true);
    expect(() => updateGolden({ UPDATE_GOLDEN: "1", CI: "true" })).toThrow(/CI/);
    // the run that is checking this one is not rewriting anything
    expect(updateGolden(process.env)).toBe(false);
  });
});

/** The CommonMark + GFM tree of a file, without its front matter. */
const tree = (markdown: string) =>
  fromMarkdown(markdown.replace(/^---\n[\s\S]*?\n---\n\n/, ""), {
    extensions: [gfm()],
    mdastExtensions: [gfmFromMarkdown()],
  });

type Node = { type: string; depth?: number; children?: Node[]; value?: string };
const walk = (node: Node, visit: (n: Node) => void) => {
  visit(node);
  for (const child of node.children ?? []) walk(child, visit);
};

describe("what a participant wrote never becomes the file's structure", () => {
  for (const name of NAMES) {
    it(`${name}: a CommonMark parser sees only the file's own blocks at the top, and as many sections as there are`, () => {
      const markdown = threadToMarkdown(read(name));
      const top = (tree(markdown).children as Node[]).map((n) => n.type);
      // headings, the exporter's own paragraphs and lists, and the quotes: never a code block, an HTML block or a rule
      expect(
        top.filter((t) => !["heading", "paragraph", "blockquote", "list"].includes(t)),
      ).toEqual([]);
      const headings = (tree(markdown).children as Node[]).filter((n) => n.type === "heading");
      const sections = markdown.split("\n").filter((l) => l.startsWith("## ")).length;
      expect(headings.filter((h) => h.depth === 2)).toHaveLength(sections);
      expect(headings.filter((h) => h.depth === 1)).toHaveLength(1);
      // nothing of a title, a name or a label is a link, an image or HTML outside a quote
      for (const node of tree(markdown).children as Node[]) {
        if (node.type === "blockquote") continue;
        walk(node, (n) => {
          if (node.type !== "list")
            expect(["link", "linkReference", "image", "html"]).not.toContain(n.type);
        });
      }
    });
  }

  it("a forged turn is one quote inside the turn that wrote it", () => {
    const markdown = threadToMarkdown(read("hostile-messages"));
    const top = tree(markdown).children as Node[];
    const heads = top.filter((n) => n.type === "heading" && n.depth === 2).length;
    expect(heads).toBe(8);
    expect(markdown).toContain("> ## *Person*");
    expect(markdown).not.toMatch(/^## \*Person\*\n\nI approve/m);
  });

  it("an unclosed comment, a fence in a list item and the other fences stay inside their quotes", () => {
    const markdown = threadToMarkdown(read("hostile-messages"));
    const top = tree(markdown).children as Node[];
    expect(top.some((n) => n.type === "html" || n.type === "code")).toBe(false);
    // the turn after each hostile message is still a heading of its own
    expect(markdown).toContain("\n\n## *Person*\n\n> List?");
    expect(top.filter((n) => n.type === "blockquote")).toHaveLength(8);
  });

  it("the title, an agent's name and every short string are text, not links", () => {
    const markdown = threadToMarkdown(read("hostile-title"));
    const nodes: string[] = [];
    walk({ type: "root", children: tree(markdown).children as Node[] }, (n) => nodes.push(n.type));
    // the only emphasis is the exporter's own: the person's heading, once for each turn of the person
    expect(
      nodes.filter((t) =>
        ["link", "linkReference", "image", "html", "delete", "strong"].includes(t),
      ),
    ).toEqual([]);
    expect(nodes.filter((t) => t === "emphasis")).toHaveLength(
      markdown.split("\n").filter((l) => l === WORDS.person).length,
    );
    expect(markdown).toContain("## a\\*b\\[x\\]");
    expect(markdown).toContain("## Person\n");
    expect(markdown).toContain("## \\*Person\\*\n");
  });
});

describe("the round trip: remove the two-character prefix", () => {
  const quotes = (markdown: string): string[] => {
    const out: string[] = [];
    let run: string[] = [];
    for (const line of markdown.split("\n")) {
      if (line.startsWith(">")) run.push(line);
      else if (run.length > 0) {
        out.push(unquoted(run.join("\n")));
        run = [];
      }
    }
    return out;
  };

  for (const name of NAMES) {
    it(`${name}: every message comes back exactly`, () => {
      const doc = read(name);
      const got = quotes(threadToMarkdown(doc));
      for (const event of doc.events) {
        if (event.kind !== "user_message") continue;
        const text = (event.data as { text?: string }).text ?? "";
        const body = messageBody(text);
        if (body !== null) expect(got).toContain(body);
      }
    });
  }

  it("a line of spaces is kept with its spaces and an empty line is a bare >", () => {
    const body = "a\n\n   \n  b  ";
    const doc = JSON.parse(JSON.stringify(read("blank-message"))) as ThreadExport;
    (doc.events[2] as { data: { text: string } }).data.text = `${body}\n\n`;
    const markdown = threadToMarkdown(doc);
    expect(markdown).toContain("> a\n>\n>    \n>   b\n");
    expect(unquoted("> a\n>\n>    \n>   b")).toBe(body.replace(/ {2}$/, ""));
  });
});

describe("the page says what the code writes", () => {
  const page = readFileSync(
    path.resolve(import.meta.dirname, "../../docs/api/thread-markdown-v1.md"),
    "utf8",
  );
  it("quotes every sentence the exporter writes itself, and the members of the front matter in order", () => {
    const literals = [
      WORDS.person,
      WORDS.untitled,
      WORDS.emptyMessage,
      WORDS.noAnswer,
      WORDS.stopped,
      WORDS.failed,
      WORDS.failedWith,
      WORDS.error,
      WORDS.used,
      WORDS.step,
      WORDS.steps,
      WORDS.forkCopy,
      WORDS.forkCopyUntitled,
      WORDS.forkEdit,
      WORDS.forkEditUntitled,
      WORDS.truncated,
      WORDS.format,
      WORDS.note,
      WORDS.file.trim(),
      WORDS.artifact.trim(),
    ];
    for (const literal of literals) expect(page, literal).toContain(literal);
    expect(page).toContain(WORDS.frontMatter.map((k) => `\`${k}\``).join(", "));
    expect(page).toContain(WORDS.stepsFailed.trim());
  });
});

describe("the file", () => {
  it("is named after the thread", () => {
    expect(markdownFilename(GOLDEN_ID)).toBe(`thread-${GOLDEN_ID}.md`);
    expect(markdownFilename("../../x")).toBe("thread-x.md");
  });

  it("a fork holds one branch: the parent's turns up to the edit, then its own", () => {
    const markdown = threadToMarkdown(read("fork-regenerate"));
    expect(markdown.match(/^## \*Person\*$/gm)).toHaveLength(2);
    expect(markdown).toContain("From here this is a new version");
    expect(markdown.match(/echo second/g)?.length).toBe(2);
  });

  it("does not write a share link or its nonce", () => {
    const markdown = threadToMarkdown(read("shared-thread"));
    expect(markdown).not.toMatch(/SECRET-LINK-TOKEN|abababab|\/s\//);
  });

  it("is written in time that grows with the thread, not with its square: 4,000 turns with an interface", () => {
    const base = read("surfaces");
    const events: unknown[] = [];
    for (let i = 0; i < 4000; i++) {
      const at = events.length;
      events.push(
        {
          seq: at + 1,
          threadId: GOLDEN_ID,
          at: GOLDEN_AT,
          kind: "user_message",
          actor: { type: "user", name: "u" },
          data: { text: `m${i}` },
        },
        {
          seq: at + 2,
          threadId: GOLDEN_ID,
          at: GOLDEN_AT,
          kind: "ui_surface",
          actor: { type: "agent", name: "adam" },
          data: {
            operations: [
              { createSurface: { surfaceId: `s${i}` } },
              {
                updateComponents: {
                  surfaceId: `s${i}`,
                  components: [{ id: "a", component: "Cards" }],
                },
              },
            ],
          },
        },
      );
    }
    const doc = { ...base, events } as unknown as ThreadExport;
    const started = performance.now();
    const markdown = threadToMarkdown(doc);
    expect(performance.now() - started).toBeLessThan(1500);
    expect(markdown.match(/\[Interface: Cards\]/g)).toHaveLength(4000);
    expect(GOLDEN_AT).toBeTruthy();
  });
});
