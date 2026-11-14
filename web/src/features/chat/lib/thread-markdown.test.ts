import { describe, expect, it } from "vitest";
import type { components } from "@/lib/api/schema";
import { SurfaceLog, surfacePlaceholders } from "./a2ui/placeholder";
import {
  codeSpan,
  inline,
  messageBody,
  quoted,
  threadToMarkdown,
  unquoted,
  utc,
} from "./thread-markdown";

type Doc = components["schemas"]["ThreadExport"];
type Ev = components["schemas"]["Event"];

const ID = "0190aaaa-0000-7000-8000-000000000123";
let seq = 0;
const ev = (
  kind: string,
  data: Record<string, unknown>,
  actor: { type: "user" | "agent" | "system"; name: string } = { type: "agent", name: "adam" },
): Ev =>
  ({ seq: ++seq, threadId: ID, at: "2026-11-14T10:00:00Z", kind, actor, data }) as unknown as Ev;
const said = (text: string) =>
  ev("user_message", { text }, { type: "user", name: "me@example.com" });
const doc = (events: Ev[], over: Partial<Doc> = {}): Doc =>
  ({
    format: "another-agentic-system/thread-export",
    version: 1,
    exportedAt: "2026-11-14T19:09:33.999Z",
    thread: { id: ID, title: "T", target: { agentId: "adam" } },
    events,
    eventsTruncated: false,
    ...over,
  }) as unknown as Doc;

describe("pieces", () => {
  it("escapes what Markdown would act on, on one line", () => {
    expect(inline("a *b*\n[c](d) <e> `f` _g_ & \\ ~h~")).toBe(
      "a \\*b\\* \\[c\\](d) \\<e\\> \\`f\\` \\_g\\_ \\& \\\\ \\~h\\~",
    );
    expect(inline("Fix the bug.")).toBe("Fix the bug.");
  });
  it("writes a word that would become a link by itself as a code span", () => {
    expect(inline("see https://evil.example now")).toBe("see `https://evil.example` now");
    expect(inline("www.x.example")).toBe("`www.x.example`");
    expect(inline("a@b.example")).toBe("`a@b.example`");
    expect(inline("HTTP://X mailto:a")).toBe("`HTTP://X` `mailto:a`");
    expect(inline("awww.x")).toBe("awww.x");
  });
  it("collapses whitespace runs of space, tab, CR and LF only", () => {
    expect(inline(" a \t\r\n b c ")).toBe("a b c");
  });
  it("makes a code span that a backtick cannot end", () => {
    expect(codeSpan("a`b")).toBe("``a`b``");
    expect(codeSpan("`x`")).toBe("`` `x` ``");
  });
  it("keeps a message as written but for its ends: line endings, leading blank lines, trailing space", () => {
    expect(messageBody("\r\n\r\n  a\r\nb  \r\n\r\n")).toBe("  a\nb");
    expect(messageBody(" ")).toBe(" ");
    expect(messageBody("  \n\t\n")).toBeNull();
    expect(messageBody("a\n \n  b  \n\n ")).toBe("a\n \n  b");
  });
  it("quotes every line, an empty one with a bare >, and the prefix comes off exactly", () => {
    const body = "a\n\n   \n  b  ";
    expect(quoted(body)).toBe("> a\n>\n>    \n>   b  ");
    expect(unquoted(quoted(body))).toBe(body);
    expect(quoted("## x\n```\n<!--")).toBe("> ## x\n> ```\n> <!--");
  });
  it("says the time in UTC to the second, cut and not rounded", () => {
    expect(utc("2026-11-14T21:09:33.999+02:00")).toBe("2026-11-14T19:09:33Z");
    expect(utc("not a date")).toBe("not a date");
  });
});

describe("threadToMarkdown", () => {
  it("writes the front matter, the title and the turns in order, every message as a quote", () => {
    const md = threadToMarkdown(
      doc([
        said("hi"),
        ev("agent_message", { messageId: "m1", final: true, text: "hello\n" }),
        said("again"),
        ev("agent_message", { messageId: "m2", final: true, text: "yes" }),
      ]),
    );
    expect(md).toBe(
      [
        "---",
        "format: another-agentic-system/thread-markdown",
        "version: 1",
        'title: "T"',
        'agent: "adam"',
        `thread: "${ID}"`,
        "exported: 2026-11-14T19:09:33Z",
        "---",
        "",
        "# T",
        "",
        "## *Person*",
        "",
        "> hi",
        "",
        "## adam",
        "",
        "> hello",
        "",
        "## *Person*",
        "",
        "> again",
        "",
        "## adam",
        "",
        "> yes",
        "",
      ].join("\n"),
    );
  });

  it("opens a section for each person event, even two in a row", () => {
    const md = threadToMarkdown(doc([said("a"), said("b")]));
    expect(md.match(/^## \*Person\*$/gm)).toHaveLength(2);
  });

  it("keeps the answer of a turn by its purpose, not by being last", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("agent_message", { messageId: "a", final: true, text: "looking", purpose: "working" }),
        ev("agent_message", {
          messageId: "b",
          final: true,
          text: "first answer",
          purpose: "answer",
        }),
        ev("agent_message", {
          messageId: "c",
          final: true,
          text: "last answer",
          purpose: "answer",
        }),
        ev("agent_message", {
          messageId: "d",
          final: true,
          text: "later words",
          purpose: "working",
        }),
      ]),
    );
    expect(md).toContain("> last answer");
    expect(md).not.toMatch(/looking|first answer|later words/);
  });

  it("reads unmarked text as ADR 0031 does: the last one is the answer", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("agent_message", { messageId: "a", final: true, text: "earlier" }),
        ev("agent_message", { messageId: "b", final: true, text: "the answer" }),
        ev("agent_message", { messageId: "c", final: true, text: "  \n" }),
      ]),
    );
    expect(md).toContain("> the answer");
    expect(md).not.toContain("earlier");
  });

  it("a message written again keeps its first place and its last words", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("agent_message", { messageId: "a", final: false, text: "par", purpose: "answer" }),
        ev("agent_message", { messageId: "b", final: true, text: "other", purpose: "answer" }),
        ev("agent_message", {
          messageId: "a",
          final: true,
          text: "partial, whole",
          purpose: "answer",
        }),
      ]),
    );
    // `b` is later by place, so it is the answer; `a`'s old words are gone
    expect(md).toContain("> other");
    expect(md).not.toMatch(/par\b|partial/);
  });

  it("counts steps and never writes what they were called with or returned", () => {
    const step = (id: string, state: string, extra = {}) =>
      ev("agent_step", {
        id,
        path: [],
        kind: "command",
        label: "curl secret.example",
        state,
        phase: "end",
        ...extra,
      });
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("agent_step", {
          id: "s1",
          path: [],
          kind: "command",
          label: "x",
          state: "running",
          phase: "start",
          input: { token: "hunter2" },
        }),
        step("s1", "completed", { output: { text: "password: hunter2" } }),
        step("s2", "failed"),
        ev("agent_step", {
          id: "n",
          path: [],
          kind: "message",
          label: "note",
          state: "completed",
          phase: "end",
        }),
        ev("agent_message", { messageId: "a", final: true, text: "done" }),
      ]),
    );
    expect(md).toContain("*2 steps, 1 failed.*");
    expect(md).not.toMatch(/hunter2|secret\.example/);
    const one = threadToMarkdown(doc([said("go"), step("s1", "completed")]));
    expect(one).toContain("*1 step.*");
  });

  it("names a file and never links it, a uri included; links https in any case", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("artifact", {
          name: "chart",
          mimeType: "image/png",
          uri: "https://example.com/private.png",
          file: { sha256: "a".repeat(64), size: 70, filename: "we`ird.png" },
        }),
        ev("artifact", { name: "patch", text: "line one\nline two", uri: "javascript:alert(1)" }),
        ev("artifact", { name: "pr", text: "Fix it", uri: "HTTPS://github.com/a/b/pull/1" }),
      ]),
    );
    expect(md).toContain("- File: ``we`ird.png`` (image/png, 70 bytes)");
    expect(md).not.toContain("private.png");
    expect(md).toContain("- Artifact: patch\n");
    expect(md).not.toMatch(/javascript|line two/);
    expect(md).toContain("- Artifact: pr — Fix it — <HTTPS://github.com/a/b/pull/1>");
  });

  it("counts the 200 of an artifact's text in Unicode scalar values", () => {
    const at = (text: string) =>
      threadToMarkdown(doc([said("go"), ev("artifact", { name: "t", text })]));
    expect(at("🎉".repeat(200))).toContain("🎉");
    expect(at("🎉".repeat(201))).not.toContain("🎉");
    expect(at("x".repeat(200))).toContain("x".repeat(200));
    expect(at("x".repeat(201))).not.toContain("xxx");
  });

  it("shows an interface as a placeholder with its questions, and the answer under the person", () => {
    const md = threadToMarkdown(
      doc([
        said("ask"),
        ev("ui_surface", {
          operations: [
            { createSurface: { surfaceId: "s1" } },
            {
              updateComponents: {
                surfaceId: "s1",
                components: [
                  { id: "root", component: "Column", children: ["pick"] },
                  {
                    id: "pick",
                    component: "Choices",
                    questions: [
                      {
                        id: "q",
                        question: "Which? *one*",
                        options: [
                          { value: "a", label: "Alpha" },
                          { value: "b", label: "Beta" },
                        ],
                      },
                    ],
                  },
                ],
              },
            },
          ],
        }),
        ev(
          "ui_action",
          {
            surfaceId: "s1",
            name: "answer",
            sourceComponentId: "pick",
            context: { answers: [{ id: "q", values: ["b"], other: "or c" }], note: "two\nlines" },
          },
          { type: "user", name: "me@example.com" },
        ),
        ev(
          "ui_action",
          { surfaceId: "s1", name: "go`x", context: {} },
          { type: "user", name: "me@example.com" },
        ),
      ]),
    );
    expect(md).toContain("*[Interface: Choices]*\n\n- Which? \\*one\\*");
    expect(md).toContain(
      "**Which? \\*one\\***\n\n> Beta, Other: or c\n\n**Note**\n\n> two\n> lines",
    );
    expect(md).toContain("*Used ``go`x`` on an interface.*");
  });

  it("marks a fork and an edit, with and without the parent's title, and says when the log was cut", () => {
    const fork = (data: Record<string, unknown>) =>
      threadToMarkdown(
        doc([said("a"), ev("thread_forked", data, { type: "user", name: "me" }), said("b")], {
          eventsTruncated: true,
        }),
      );
    expect(fork({ kind: "fork", title: "Parent *x*" })).toContain(
      '*The conversation above was copied from the chat "Parent \\*x\\*"; what follows was said here.*',
    );
    expect(fork({ kind: "fork" })).toContain(
      "*The conversation above was copied from another chat; what follows was said here.*",
    );
    expect(fork({ kind: "edit", title: "  " })).toContain(
      "*From here this is a new version: a message of an earlier chat was edited.*",
    );
    expect(fork({ kind: "edit", title: "P" })).toContain('a message of the chat "P" was edited.*');
    expect(fork({ kind: "fork" })).toContain("it holds the first 3 events.*");
  });

  it("writes a failure, an error and a stop in words", () => {
    const md = threadToMarkdown(
      doc([
        said("a"),
        ev("agent_status", { status: "failed", detail: "model refused" }),
        said("b"),
        ev("agent_status", { status: "failed" }),
        said("c"),
        ev("agent_status", { status: "canceled" }),
        said("d"),
        ev("error", { message: "boom" }, { type: "system", name: "orchestrator" }),
      ]),
    );
    expect(md).toContain("*The agent failed: model refused*");
    expect(md).toContain("*The agent failed.*");
    expect(md).toContain("*The turn was stopped.*");
    expect(md).toContain("*Error: boom*");
  });

  it("takes the name of the turn's first agent, and `agent` when there is none", () => {
    const md = threadToMarkdown(
      doc([
        said("a"),
        ev("agent_message", { messageId: "1", text: "x" }, { type: "agent", name: "alpha" }),
        ev("agent_message", { messageId: "2", text: "y" }, { type: "agent", name: "beta" }),
      ]),
    );
    expect(md).toContain("## alpha");
    expect(md).not.toContain("## beta");
    const none = threadToMarkdown(
      doc([ev("error", { message: "x" }, { type: "system", name: "orchestrator" })], {
        thread: { id: ID, title: "T", target: { agentId: "" } } as Doc["thread"],
      }),
    );
    expect(none).toContain("## agent");
  });

  it("writes an empty title as Untitled chat and keeps non-ASCII as written", () => {
    const md = threadToMarkdown(
      doc([], {
        thread: {
          id: ID,
          title: "",
          description: "Café 日本語",
          target: { agentId: "adam" },
        } as Doc["thread"],
      }),
    );
    expect(md).toContain("\n# Untitled chat\n");
    expect(md).toContain('title: ""');
    expect(md).toContain('description: "Café 日本語"');
  });

  it("makes a title that cannot be a link or a second heading", () => {
    const md = threadToMarkdown(
      doc([], {
        thread: {
          id: ID,
          title: "[x](javascript:1)\n## no",
          target: { agentId: "adam" },
        } as Doc["thread"],
      }),
    );
    expect(md).toContain("# \\[x\\](javascript:1) ## no\n");
    expect(md).toContain('title: "[x](javascript:1) ## no"');
  });

  it("writes an empty message as a placeholder", () => {
    const md = threadToMarkdown(doc([said("  \n\t"), said("x")]));
    expect(md).toContain("## *Person*\n\n*Empty message.*\n\n## *Person*\n\n> x");
  });
});

describe("a thread of surfaces is read once, not once for each turn", () => {
  it("files operations by surface, in the order the surfaces were named", () => {
    const log = new SurfaceLog();
    expect(
      log.feed([
        { createSurface: { surfaceId: "a" } },
        { createSurface: { surfaceId: "b" } },
        { updateComponents: { surfaceId: "a", components: [{ id: "c", component: "Cards" }] } },
      ]),
    ).toEqual(["a", "b"]);
    expect(log.operations("a")).toHaveLength(2);
    expect(log.types("a")).toEqual(["Cards"]);
    expect(log.types("zzz")).toEqual([]);
  });
});

describe("the placeholder of a surface", () => {
  const surface = (...types: string[]) => [
    { createSurface: { surfaceId: "s" } },
    {
      updateComponents: {
        surfaceId: "s",
        components: types.map((component, i) => ({ id: `c${i}`, component })),
      },
    },
  ];
  it("names what the surface is about, not its layout", () => {
    expect(surfacePlaceholders(surface("Column", "Text", "Cards", "Mermaid", "Cards"))).toEqual([
      "[Interface: Cards, Mermaid]",
    ]);
    expect(surfacePlaceholders(surface("Column", "Text"))).toEqual(["[Interface: Text]"]);
    expect(surfacePlaceholders(surface("Column", "Stack"))).toEqual(["[Interface: Column, Stack]"]);
    expect(surfacePlaceholders(surface())).toEqual(["[Interface]"]);
    expect(surfacePlaceholders([])).toEqual([]);
  });
  it("starts a surface over at createSurface and drops a deleted one", () => {
    expect(surfacePlaceholders([...surface("Cards"), ...surface("Chart")])).toEqual([
      "[Interface: Chart]",
    ]);
    expect(
      surfacePlaceholders([...surface("Cards"), { deleteSurface: { surfaceId: "s" } }]),
    ).toEqual([]);
  });
  it("keeps only letters and digits of an agent's word", () => {
    expect(surfacePlaceholders(surface("Ca]rds\n**"))).toEqual(["[Interface: Cards]"]);
  });
});
