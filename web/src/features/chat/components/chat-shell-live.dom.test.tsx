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
 * Live text (ADR 0027) through the whole app: the mock relays the pieces of a reply as the
 * orchestrator does (frames with `metadata["vymalo.live"]`, never in the log), `ThreadAgent` keeps
 * them as drafts out of the runtime, and the turn draws them; the log's message, when it comes, is
 * the one reply. The mock plays a step every 250 ms, slow enough to see the words grow (a viewer hears only
 * the pieces that come after it connects, so the app is warmed up before a thread starts).
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

const log = () => screen.getByRole("log", { name: "Conversation" });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const drafts = () => [...log().querySelectorAll<HTMLElement>('[data-slot="agent-draft"]')];
const replies = () => [...log().querySelectorAll<HTMLElement>('[data-slot="agent-message"]')];
/** How many times `words` is in the conversation, drafts and replies alike. */
const times = (words: string) => (log().textContent ?? "").split(words).length - 1;

describe("live text, in the app", () => {
  it("stream-long: the words grow in the turn as a draft, then the log's message is the one reply and no draft is left", async () => {
    const id = await makeThread("stream-long write the plan");
    shell(id);

    // a draft in the agent's turn, with the first words, busy and silent for a screen reader
    await waitFor(() => expect(drafts()).toHaveLength(1));
    const draft = drafts()[0] as HTMLElement;
    expect(draft.getAttribute("aria-busy")).toBe("true");
    expect(draft.getAttribute("aria-live")).toBe("off");
    expect(draft.closest('[data-slot="agent-turn"]')).not.toBeNull();
    // not the finished reply: that is the runtime's message, which the log does not have yet
    expect(replies()).toHaveLength(0);
    const first = draft.textContent ?? "";
    expect(first.startsWith("I'll start with the failing test")).toBe(true);

    // it grows
    await waitFor(() =>
      expect((drafts()[0]?.textContent ?? "").length).toBeGreaterThan(first.length),
    );
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("that fixes it."));
    // Markdown as it is written: the list is a list
    await waitFor(() => expect(drafts()[0]?.querySelectorAll("li").length).toBeGreaterThan(0));

    // the log says the reply: one message, the draft is gone, the words are there once
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(drafts()).toHaveLength(0));
    expect(replies()).toHaveLength(1);
    expect(replies()[0]?.textContent).toContain("when it is green.");
    expect(replies()[0]?.querySelectorAll("li")).toHaveLength(3);
    expect(times("I'll start with the failing test")).toBe(1);
    expect(times("when it is green.")).toBe(1);
  });

  it("stream: the golden's three pieces, one message at the end", async () => {
    const id = await makeThread("stream write fibonacci in rust");
    shell(id);
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("Fib"));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(drafts()).toHaveLength(0));
    expect(replies().map((r) => r.textContent)).toEqual(["Fibonacci in Rust."]);
    expect(times("Fibonacci in Rust.")).toBe(1);
  });

  it("a reply that is still being written is a draft the transcript does not hold, and Stop ends it", async () => {
    const id = await makeThread("stream-hold write the plan");
    shell(id);
    await waitFor(() =>
      expect(drafts()[0]?.textContent).toContain("then make the smallest change"),
    );
    expect(replies()).toHaveLength(0);
    // the turn says it works: the draft stands in place of the starting line
    expect(within(log()).queryByText(/is starting/)).toBeNull();

    fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
    // given up with the run: nothing is left on screen that looks alive
    await waitFor(() => expect(drafts()).toHaveLength(0));
    expect(times("then make the smallest change")).toBe(0);
  });

  it("a stream that is given up goes, and the words the agent says next are said once", async () => {
    const id = await makeThread("stream-abandon what is the answer");
    shell(id);
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("The answer is forty-"));
    await waitFor(() => expect(drafts()).toHaveLength(0));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(replies().map((r) => r.textContent)).toEqual([
      "Sorry, let me say that again: it is forty-two.",
    ]);
    expect(times("The answer is")).toBe(0);
    expect(times("it is forty-two.")).toBe(1);
  });

  it("a connection cut mid-reply: the new one has the log only, and the reply arrives once, whole", async () => {
    const id = await makeThread("stream-long write the plan");
    shell(id);
    await waitFor(() => expect(drafts()).toHaveLength(1));
    // the network went away: every open stream is cut; the drafts go with the connection
    await realFetch(`${base}/__mock/drop-streams`, { method: "POST" });
    await waitFor(() => expect(drafts()).toHaveLength(0));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(replies()).toHaveLength(1));
    expect(replies()[0]?.textContent).toContain("when it is green.");
    expect(drafts()).toHaveLength(0);
    expect(times("I'll start with the failing test")).toBe(1);
  });

  it("a page opened mid-reply is told the text so far by the sender's refresh, from the start, once", async () => {
    const id = await makeThread("stream-hold write the plan");
    shell(id);
    await waitFor(() =>
      expect(drafts()[0]?.textContent).toContain("then make the smallest change"),
    );
    cleanup(); // the tab is closed
    shell(id); // and opened again: its connection has heard none of the pieces
    await waitFor(() => expect(drafts()).toHaveLength(1));
    expect(drafts()[0]?.textContent).toContain("I'll start with the failing test");
    expect(times("I'll start with the failing test")).toBe(1);
    // the same words, not a second copy of them, as the refresh comes round again
    await new Promise((r) => setTimeout(r, 900));
    expect(drafts()).toHaveLength(1);
    expect(times("I'll start with the failing test")).toBe(1);
    fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
  });

  it("a thread opened after the reply read the plain message: no draft, once", async () => {
    const id = await makeThread("stream write fibonacci in rust");
    // let the run finish before anyone looks
    for (let i = 0; i < 100; i++) {
      const t = (await (await realFetch(`${base}/api/threads/${id}`)).json()) as { state: string };
      if (t.state === "done") break;
      await new Promise((r) => setTimeout(r, 50));
    }
    shell(id);
    await waitFor(() => expect(replies()).toHaveLength(1));
    expect(drafts()).toHaveLength(0);
    expect(times("Fibonacci in Rust.")).toBe(1);
  });
});
