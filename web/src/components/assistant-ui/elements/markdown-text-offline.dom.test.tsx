// @vitest-environment jsdom
import {
  AssistantRuntimeProvider,
  type ThreadMessageLike,
  ThreadPrimitive,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AssistantMessage } from "./thread.aui";

/*
 * Math and highlighting are lazy chunks. Offline, or when a chunk is gone after a deploy, the import rejects: the
 * answer must stay as it was written, and nothing may throw (web/README.md "Code and math in answers"). Here both
 * imports reject: the chunk of math, and the highlighter's core.
 */

const attempts = vi.hoisted(() => ({ math: 0, shiki: 0 }));

vi.mock("@/lib/math", () => {
  attempts.math++;
  throw new Error("Failed to fetch dynamically imported module (offline)");
});
vi.mock("shiki/core", () => {
  attempts.shiki++;
  throw new Error("Failed to fetch dynamically imported module (offline)");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

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

describe("MarkdownText offline", () => {
  it("shows a formula and a block of code as written when the chunks cannot be loaded, and throws nothing", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const unhandled: unknown[] = [];
    const onUnhandled = (event: PromiseRejectionEvent) => unhandled.push(event.reason);
    window.addEventListener("unhandledrejection", onUnhandled);
    try {
      render(<Harness text={"Before.\n\n$$\nx^2\n$$\n\n```rust\nfn main() {}\n```\n\nAfter."} />);
      const log = await screen.findByRole("log", { name: "Conversation" });
      await waitFor(() => expect(log.querySelector("pre code")).not.toBeNull());
      // both imports were tried, and both were refused
      await waitFor(() => expect(attempts.math).toBeGreaterThan(0));
      await waitFor(() => expect(attempts.shiki).toBeGreaterThan(0));
      await new Promise((r) => setTimeout(r, 300));

      expect(log.querySelector(".katex, .katex-error, .katex-display")).toBeNull();
      expect(log.textContent).toContain("Before.");
      expect(log.textContent).toContain("After.");
      // the formula is the text that was written, with its fences
      expect(log.textContent).toMatch(/\$\$\s*x\^2\s*\$\$/);
      const code = log.querySelector("pre code") as HTMLElement;
      expect(code.textContent).toBe("fn main() {}\n");
      expect(code.querySelector("[data-token]")).toBeNull();
      expect(errors).not.toHaveBeenCalled();
      expect(unhandled).toEqual([]);
    } finally {
      window.removeEventListener("unhandledrejection", onUnhandled);
    }
  });
});
