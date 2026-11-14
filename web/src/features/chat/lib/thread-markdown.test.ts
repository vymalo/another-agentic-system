import { describe, expect, it } from "vitest";
import type { components } from "@/lib/api/schema";
import { surfacePlaceholders } from "./a2ui/placeholder";
import { codeSpan, inline, messageBody, threadToMarkdown, utc } from "./thread-markdown";

type Doc = components["schemas"]["ThreadExport"];
type Ev = components["schemas"]["Event"];

const ID = "0190aaaa-0000-7000-8000-000000000123";
let seq = 0;
const ev = (
  kind: string,
  data: Record<string, unknown>,
  actor: { type: "user" | "agent" | "system"; name: string } = { type: "agent", name: "adam" },
): Ev =>
  ({
    seq: ++seq,
    threadId: ID,
    at: "2026-11-14T10:00:00Z",
    kind,
    actor,
    data,
  }) as unknown as Ev;
const said = (text: string) =>
  ev("user_message", { text }, { type: "user", name: "me@example.com" });
const doc = (events: Ev[], over: Partial<Doc> = {}): Doc =>
  ({
    format: "another-agentic-system/thread-export",
    version: 1,
    exportedAt: "2026-11-14T19:09:33.456Z",
    thread: { id: ID, title: "T", target: { agentId: "adam" } },
    events,
    eventsTruncated: false,
    ...over,
  }) as unknown as Doc;

describe("pieces", () => {
  it("escapes what Markdown would act on, on one line", () => {
    expect(inline("a *b*\n[c](d) <e> `f` _g_ & \\")).toBe(
      "a \\*b\\* \\[c\\](d) \\<e\\> \\`f\\` \\_g\\_ \\& \\\\",
    );
  });
  it("makes a code span that a backtick cannot end", () => {
    expect(codeSpan("a`b")).toBe("``a`b``");
    expect(codeSpan("`x`")).toBe("`` `x` ``");
  });
  it("writes a message with LF line endings and no blank lines around it", () => {
    expect(messageBody("\r\n\r\n  a\r\nb\r\n\r\n")).toBe("  a\nb");
  });
  it("closes a code fence the message left open, so what follows is not swallowed", () => {
    expect(messageBody("x\n```rust\nfn a() {}")).toBe("x\n```rust\nfn a() {}\n```");
    expect(messageBody("~~~~\ncode\n~~~")).toBe("~~~~\ncode\n~~~\n~~~~");
    expect(messageBody("```\na\n```\nb")).toBe("```\na\n```\nb");
    expect(messageBody("``` `x`\nstill text")).toBe("``` `x`\nstill text");
  });
  it("says the time in UTC to the second", () => {
    expect(utc("2026-11-14T21:09:33.456+02:00")).toBe("2026-11-14T19:09:33Z");
    expect(utc("not a date")).toBe("not a date");
  });
});

describe("threadToMarkdown", () => {
  it("writes the front matter, the title and the turns in order", () => {
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
        "## Person",
        "",
        "hi",
        "",
        "## adam",
        "",
        "hello",
        "",
        "## Person",
        "",
        "again",
        "",
        "## adam",
        "",
        "yes",
        "",
      ].join("\n"),
    );
  });

  it("keeps the answer of a turn and drops its working words, by the purpose the log gives", () => {
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
      ]),
    );
    expect(md).toContain("last answer");
    expect(md).not.toContain("looking");
    expect(md).not.toContain("first answer");
  });

  it("reads unmarked text as ADR 0031 does: the last one is the answer", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("agent_message", { messageId: "a", final: true, text: "earlier" }),
        ev("agent_message", { messageId: "b", final: true, text: "the answer" }),
      ]),
    );
    expect(md).toContain("the answer");
    expect(md).not.toContain("earlier");
  });

  it("takes the last version of a message that was written in pieces", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("agent_message", { messageId: "a", final: false, text: "par" }),
        ev("agent_message", { messageId: "a", final: true, text: "partial, whole" }),
      ]),
    );
    expect(md).toContain("partial, whole");
    expect(md).not.toMatch(/^par$/m);
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
  });

  it("names files, never their bytes or a link, and links an https artifact", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("artifact", {
          name: "chart",
          mimeType: "image/png",
          file: { sha256: "a".repeat(64), size: 70, filename: "we`ird.png" },
        }),
        ev("artifact", { name: "patch", text: "line one\nline two", uri: "javascript:alert(1)" }),
        ev("artifact", { name: "pr", text: "Fix it", uri: "https://github.com/a/b/pull/1" }),
      ]),
    );
    expect(md).toContain("- File: ``we`ird.png`` (image/png, 70 bytes)");
    expect(md).toContain("- Artifact: patch\n");
    expect(md).not.toMatch(/javascript|line two|a{64}/);
    expect(md).toContain("- Artifact: pr — Fix it — <https://github.com/a/b/pull/1>");
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
            context: { answers: [{ id: "q", values: ["b"], other: "or c" }] },
          },
          { type: "user", name: "me@example.com" },
        ),
      ]),
    );
    expect(md).toContain("*[Interface: Choices]*\n\n- Which? \\*one\\*");
    expect(md).toContain("- **Which? \\*one\\*** Beta, Other: or c");
  });

  it("marks a fork and an edit, and says when the log was cut", () => {
    const fork = threadToMarkdown(
      doc(
        [
          said("a"),
          ev(
            "thread_forked",
            { kind: "fork", title: "Parent *x*", from: { seq: 1 } },
            { type: "user", name: "me" },
          ),
          said("b"),
        ],
        { eventsTruncated: true },
      ),
    );
    expect(fork).toContain(
      '*The conversation above was copied from the chat "Parent \\*x\\*"; what follows was said here.*',
    );
    expect(fork).toContain("it holds the first 3 events");
  });

  it("writes a failure and a stop in words", () => {
    const md = threadToMarkdown(
      doc([
        said("a"),
        ev("agent_status", { status: "failed", detail: "model refused" }),
        said("b"),
        ev("agent_status", { status: "canceled" }),
      ]),
    );
    expect(md).toContain("*The agent failed: model refused*");
    expect(md).toContain("*The turn was stopped.*");
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

  it("writes the message source as the agent wrote it, headings included", () => {
    const md = threadToMarkdown(
      doc([
        said("go"),
        ev("agent_message", { messageId: "a", final: true, text: "# Heading\n\n<b>raw</b>" }),
      ]),
    );
    expect(md).toContain("\n# Heading\n\n<b>raw</b>\n");
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
