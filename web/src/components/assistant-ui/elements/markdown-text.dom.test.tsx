// @vitest-environment jsdom
import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AssistantMessage } from "@/features/chat/components/messages";
import { type ChatItem, convertMessage } from "@/features/chat/lib/to-items";

afterEach(cleanup);

/** Renders `text` as an agent message through the real message component (no viewport). */
function Harness({ text }: { text: string }) {
  const item: ChatItem = {
    id: "msg-1",
    seq: 1,
    at: "2026-09-29T09:00:00Z",
    actor: { type: "agent", name: "coder" },
    kind: "agent_text",
    text,
    final: true,
  };
  const runtime = useExternalStoreRuntime<ChatItem>({
    messages: [item],
    convertMessage,
    isRunning: false,
    onNew: async () => {},
  });
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ThreadPrimitive.Root>
        <div role="log" aria-label="Conversation">
          <ThreadPrimitive.Messages>{() => <AssistantMessage />}</ThreadPrimitive.Messages>
        </div>
      </ThreadPrimitive.Root>
    </AssistantRuntimeProvider>
  );
}

async function renderAgentText(text: string): Promise<HTMLElement> {
  render(<Harness text={text} />);
  const log = await screen.findByRole("log", { name: "Conversation" });
  await waitFor(() => expect(log.querySelector(".md")).not.toBeNull());
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
});
