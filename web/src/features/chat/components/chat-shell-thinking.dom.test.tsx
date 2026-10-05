// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import {
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { uuidv7 } from "@/lib/uuid";
import { createMockServer } from "../../../../mock/server";

/*
 * Reasoning (ADR 0044) through the whole app: the mock relays what the model thinks as the orchestrator
 * does (`REASONING_*` frames marked `vymalo.live`, never in the log), `ThreadAgent` keeps it as a draft out
 * of the runtime, and the turn draws it as a closed "Thinking" block above the words; the log's reasoning,
 * when it comes, is a reasoning part of the same turn, drawn by the same block. The mock plays a step every
 * 250 ms (a viewer hears only the pieces that come after it connects, so the app is warmed up first).
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 250, keepaliveMs: 1000, refreshMs: 400 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 20_000 });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};

  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const abs = (input: unknown) =>
    typeof input === "string" && input.startsWith("/") ? base + input : input;
  globalThis.Request = class extends RealRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(abs(input) as RequestInfo, init);
    }
  } as typeof Request;
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) =>
    realFetch(abs(input) as RequestInfo, init)) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
  // the first render of the app is slow (lazy chunks, cold caches): do it before a reply starts
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  render(
    <TooltipProvider>
      <ChatShell threadId={uuidv7()} />
    </TooltipProvider>,
  );
  await screen.findByText(/Thread not found/);
  cleanup();
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
beforeEach(() => {
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
});
afterEach(cleanup);

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

/** A thread made the way any AG-UI client makes one; returns once its run has started. */
async function makeThread(text: string, agent = "reviewer"): Promise<string> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/${agent}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
    }),
  });
  await res.body?.cancel();
  return threadId;
}

/**
 * Lets a held reply (`stream-gate`, `stream-abandon`) go on. The mock says 409 until the reply has
 * come to the place where it waits, which is what this waits for.
 */
async function release(threadId: string) {
  await waitFor(async () => {
    const released = await realFetch(`${base}/__mock/release?thread=${threadId}`, {
      method: "POST",
    });
    expect(released.status).toBe(204);
  });
}

const log = () => screen.getByRole("log", { name: "Conversation" });
const blocks = () => [...log().querySelectorAll<HTMLElement>('[data-slot="thinking"]')];
const reasoningText = (block: HTMLElement) =>
  block.querySelector<HTMLElement>('[data-slot="thinking-text"]')?.textContent ?? null;
const toggle = (block: HTMLElement) =>
  within(block).getByRole("button", { name: "Thinking" }) as HTMLButtonElement;

const WHOLE =
  "The user wants Fibonacci in Rust. I should write the iterative version, since it needs no recursion, and then say how it works.";

describe("reasoning, in the app", { timeout: 60_000 }, () => {
  it("think-gate: a closed Thinking block above the words streams while it is open, and stays open when the log says it", async () => {
    const id = await makeThread("think-gate write fibonacci");
    shell(id);

    // while the model thinks: one block, closed, marked as streaming, with no text in the page
    await waitFor(() => expect(blocks()).toHaveLength(1));
    const block = blocks()[0] as HTMLElement;
    expect(block.getAttribute("data-state")).toBe("closed");
    expect(block.getAttribute("data-streaming")).toBe("true");
    expect(toggle(block).getAttribute("aria-expanded")).toBe("false");
    expect(reasoningText(block)).toBeNull();
    expect(log().textContent ?? "").not.toContain("The user wants Fibonacci");
    // it is in the agent's turn, and no reply is drawn yet
    expect(block.closest('[data-slot="agent-turn"]')).not.toBeNull();

    // opened, it shows what has been thought so far
    fireEvent.click(toggle(block));
    await waitFor(() => expect(blocks()[0]?.getAttribute("data-state")).toBe("open"));
    await waitFor(() =>
      expect(reasoningText(blocks()[0] as HTMLElement)).toBe(
        "The user wants Fibonacci in Rust. I should write the iterative version, ",
      ),
    );

    // and grows while it is open when the model goes on
    await release(id);
    await waitFor(() => expect(reasoningText(blocks()[0] as HTMLElement)).toBe(WHOLE));

    // the log's reasoning and the reply arrive: one block still, still open, now not streaming, above the reply
    await waitFor(() =>
      expect(log().querySelector('[data-slot="agent-message"]')?.textContent).toContain(
        "Fibonacci in Rust.",
      ),
    );
    await waitFor(() => expect(blocks()).toHaveLength(1));
    const done = blocks()[0] as HTMLElement;
    expect(done.getAttribute("data-state")).toBe("open");
    expect(done.getAttribute("data-streaming")).toBe("false");
    expect(reasoningText(done)).toBe(WHOLE);
    const reply = log().querySelector('[data-slot="agent-message"]') as HTMLElement;
    expect(reply.textContent).toContain("Fibonacci in Rust.");
    expect(done.compareDocumentPosition(reply) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // the reasoning is in no reply: the answer says only the answer
    expect(reply.textContent).not.toContain("iterative");
  });

  it("think: a reload shows the reasoning from the log, closed, above the reply", async () => {
    const id = await makeThread("think write fibonacci");
    const first = shell(id);
    await waitFor(() => expect(screen.getByText("Fibonacci in Rust.")).toBeTruthy());
    first.unmount();

    // a viewer that comes later hears no live piece: the log's group is all there is
    shell(id);
    await waitFor(() => expect(screen.getByText("Fibonacci in Rust.")).toBeTruthy());
    await waitFor(() => expect(blocks()).toHaveLength(1));
    const block = blocks()[0] as HTMLElement;
    expect(block.getAttribute("data-state")).toBe("closed");
    expect(block.getAttribute("data-streaming")).toBe("false");
    expect(reasoningText(block)).toBeNull();
    fireEvent.click(toggle(block));
    await waitFor(() => expect(reasoningText(blocks()[0] as HTMLElement)).toBe(WHOLE));
  });

  it("think-cut: a reasoning the log cut says so", async () => {
    const id = await makeThread("think-cut write fibonacci");
    shell(id);
    await waitFor(() => expect(screen.getByText("Done.")).toBeTruthy());
    await waitFor(() => expect(blocks()).toHaveLength(1));
    fireEvent.click(toggle(blocks()[0] as HTMLElement));
    await waitFor(() =>
      expect(reasoningText(blocks()[0] as HTMLElement)).toContain(
        "[the rest of the reasoning was not kept]",
      ),
    );
  });

  it("a thread whose model did not think has no Thinking block", async () => {
    const id = await makeThread("stream write fibonacci");
    shell(id);
    await waitFor(() => expect(screen.getByText("Fibonacci in Rust.")).toBeTruthy());
    expect(blocks()).toHaveLength(0);
  });
});
